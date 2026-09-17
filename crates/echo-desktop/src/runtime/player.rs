//! Playback assembly for the desktop runtime (task 10.6 / 11.1).
//!
//! This module owns the **non-Tauri** wiring of the playback subsystem: it
//! builds the SongId→path resolver over the repository, spawns the libmpv
//! actor, constructs the [`PlaybackCoordinator`], and maps a raw
//! [`PlayerSnapshot`] into the UI-facing `UiPlayerSnapshot` (the shape the
//! frontend `playerStore` renders). Keeping Tauri types out makes every piece
//! headlessly unit-testable, mirroring `runtime::app::assemble` and
//! `platform::status_menu`.
//!
//! The thin `#[tauri::command]` layer lives in the app shell (`commands.rs`);
//! it calls into [`PlayerController`] and the snapshot mapping here.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, RwLock};

use echo_core::application::scan::ScanDeps;
use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;
use echo_core::error::Error;

use crate::player::actor::{FfiSpawnError, PlayerActor, SongResolver};
use crate::player::coordinator::PlaybackCoordinator;
use crate::player::fake::FakePlayer;
use crate::player::port::{PlayMode, PlayerPort, PlayerSnapshot, VOLUME_EPSILON};
use crate::player::queue::{QueueEntry, QueueItem, ViewContext};
use crate::player::session::{rebuild_queue, snapshot_queue, SessionPersistence};

/// One entry of the playback queue, as the queue panel renders it (task 11.2).
///
/// It carries the stable `entry_id` (queue identity, distinct from `song_id` so
/// a repeated song appears as independent entries), the `song_id` for library
/// songs or the session-only `title` for temporary items, whether it is the
/// current entry, and whether it failed to load/decode this round (error state).
/// Everything is derived from the authoritative queue — never fabricated.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiQueueEntry {
    pub entry_id: String,
    pub song_id: Option<String>,
    /// The display title. For a library entry it is the resolved song title
    /// (metadata); for a temporary item the session-provided display name.
    pub title: Option<String>,
    pub is_current: bool,
    pub failed: bool,
    /// A restored library entry whose file or root is currently unavailable.
    /// It remains visible and is retried only when availability returns.
    pub blocked: bool,
    /// True when this is a session-only temporary item (no `song_id`) that can
    /// be imported into the active library (task 11.7). Derived; never false
    /// for a library entry.
    pub can_import: bool,
    /// The entry's artist — resolved library metadata; `None` for temporary
    /// items or a library song without an artist.
    pub artist: Option<String>,
    /// The entry's duration in seconds — library metadata; `None` for
    /// temporary items or when the duration is unknown.
    pub duration_s: Option<u64>,
    /// The entry's opaque `cover://` asset key (design §16); `None` when the
    /// song carries no embedded artwork. Never an absolute path.
    pub cover_key: Option<String>,
}

/// The resolved presentation metadata of one library queue entry (task 2.2).
///
/// This is what the queue panel needs beyond the queue identity itself:
/// title/artist/duration from the library catalog plus the opaque cover
/// asset key. It is resolved in batches and cached per song so the 10 Hz
/// position stream never triggers per-row metadata queries (design: 队列变更
/// 时缓存/批量解析, 高频位置事件复用已解析的队列 DTO).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueueEntryMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_s: Option<u64>,
    pub cover_key: Option<String>,
}

/// Resolves and caches presentation metadata for library queue entries.
///
/// The resolver sits between the snapshot forwarder and the library: each
/// snapshot asks for the *current* set of song ids; the resolver answers from
/// its per-song cache and only queries the repository for ids it has not seen
/// (a queue change). Metadata for an id that no longer resolves (deleted /
/// foreign) is cached as `None` so a transient message cannot trigger a query
/// storm, and it stays that way until the process is restarted or the entry
/// is re-added to a fresh context.
pub struct QueueMetadataResolver {
    songs: Arc<dyn echo_core::application::ports::SongRepository>,
    covers: Arc<dyn echo_core::application::ports::CoverRepository>,
    // An unknown/deleted id is cached as `QueueEntryMeta::default()`, whose
    // fields serialize as explicit nulls. Keeping one value type avoids a
    // second "not found" representation and makes cache hits unambiguous.
    cache: std::sync::Mutex<std::collections::HashMap<SongId, QueueEntryMeta>>,
}

impl QueueMetadataResolver {
    /// A resolver over the same repository pair `AppServices` uses.
    #[must_use]
    pub fn new(
        songs: Arc<dyn echo_core::application::ports::SongRepository>,
        covers: Arc<dyn echo_core::application::ports::CoverRepository>,
    ) -> Self {
        Self {
            songs,
            covers,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Resolve the metadata of every library id in `queue`, batch-cached.
    ///
    /// Misses are resolved once and cached (as `QueueEntryMeta::default()` —
    /// explicit nulls — for ids that do not resolve); hits are answered from
    /// the cache, so repeated 10 Hz snapshots of an unchanged queue cost zero
    /// repository queries. Temporary items (no `song_id`) are never queried
    /// and never appear in the map (they already carry a display title).
    pub fn resolve(
        &self,
        queue: &[QueueEntry],
    ) -> std::collections::HashMap<SongId, QueueEntryMeta> {
        let mut result = std::collections::HashMap::new();
        let mut missing = std::collections::HashSet::new();
        {
            let cache = self.cache.lock().expect("metadata cache lock");
            for entry in queue {
                if let QueueItem::Library(id) = entry.item {
                    match cache.get(&id) {
                        Some(meta) => {
                            result.insert(id, meta.clone());
                        }
                        // Duplicate songs are distinct queue entries but share
                        // one library metadata record, so resolve each song id
                        // at most once per snapshot.
                        None => {
                            missing.insert(id);
                        }
                    }
                }
            }
        }

        // Resolve only the ids we have never seen (a queue change) — the
        // batched part of the contract. Deletion race: an entry dropped from
        // the library mid-queue resolves to explicit nulls, never an error.
        if !missing.is_empty() {
            let mut cache = self.cache.lock().expect("metadata cache lock");
            for id in &missing {
                let song = self.songs.by_id(*id).ok().flatten();
                let cover = song
                    .as_ref()
                    .and_then(|song| self.covers.cover_of(song.id()).ok().flatten());
                let meta = song
                    .map(|song| QueueEntryMeta {
                        title: song.title().map(ToOwned::to_owned),
                        artist: song.artist().map(ToOwned::to_owned),
                        duration_s: song.duration().map(|d| d.as_secs()),
                        cover_key: cover.map(|cover| cover.asset_key),
                    })
                    .unwrap_or_default();
                cache.insert(*id, meta.clone());
                result.insert(*id, meta);
            }
        }
        result
    }
}

/// The UI-facing playback snapshot (mirrors the frontend `UiPlayerSnapshot`).
/// Every value is derived from the authoritative [`PlayerSnapshot`] + the
/// coordinator's queue — never fabricated.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPlayerSnapshot {
    pub state: &'static str,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub volume: f64,
    pub muted: bool,
    pub current_queue_entry_id: Option<String>,
    pub current_song_id: Option<String>,
    pub queue_len: usize,
    pub mode: &'static str,
    /// The current queue entry's display fields, for the player bar. `None`
    /// when nothing is current.
    pub current_title: Option<String>,
    /// True when the current entry is a session-only temporary item that can be
    /// imported into the active library (task 11.7).
    pub current_can_import: bool,
    /// The full queue the panel renders (current + pending, in play order).
    pub queue: Vec<UiQueueEntry>,
}

/// A headless handle to the live playback subsystem: the coordinator (guarded
/// for interior mutability) and the player port (for direct commands).
pub struct PlayerController<P: PlayerPort = Arc<dyn PlayerPort>> {
    /// The coordinator, locked per command. `PlaybackCoordinator` is not
    /// internally-synchronized; the Tauri command layer locks this.
    pub coordinator: Arc<Mutex<PlaybackCoordinator<P>>>,
    /// The player port for direct commands (play/pause/seek/volume) and the
    /// snapshot stream the forwarder subscribes to.
    pub port: Arc<dyn PlayerPort>,
}

impl PlayerController {
    /// Build the resolver over the repository for the playback assembly.
    /// Mirrors `AppServices::reveal_song`: `Song::root()` → `LibraryRoot
    /// .absolute_path()` joined with `Song::path().normalized()`. The absolute
    /// path is consumed only on the actor thread and never returned to a DTO.
    pub fn resolver(deps: &Arc<ScanDeps>) -> SongResolver {
        let songs = deps.songs.clone();
        let roots = deps.roots.clone();
        Arc::new(move |song_id| {
            let song = songs
                .by_id(song_id)?
                .ok_or_else(|| Error::unavailable("song", "unknown song"))?;
            let root_id = song.root();
            let root = roots
                .by_id(root_id)?
                .ok_or_else(|| Error::unavailable("library", "song root unknown"))?;
            Ok(root.absolute_path().join(song.path().normalized()))
        })
    }
}

impl PlayerController<Arc<dyn PlayerPort>> {
    /// Spawn the real libmpv actor and build the coordinator over it.
    ///
    /// # Errors
    ///
    /// [`FfiSpawnError::Spawn`] if the actor thread cannot be created. If
    /// libmpv itself cannot load, the actor starts degraded (`Stopped`) rather
    /// than aborting.
    pub fn spawn_mpv(
        libmpv_path: &std::path::Path,
        resolver: SongResolver,
    ) -> Result<Self, FfiSpawnError> {
        let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
        let actor = PlayerActor::spawn_mpv(libmpv_path, snapshot, Some(resolver))?;
        let port: Arc<dyn PlayerPort> = Arc::new(actor);
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        Ok(Self { coordinator, port })
    }
}

impl PlayerController<Arc<dyn PlayerPort>> {
    /// A headless controller over a [`FakePlayer`] for tests and any runtime
    /// that must drive the coordinator without libmpv. The fake is shared
    /// (via `Arc<dyn PlayerPort>`) between the coordinator and the port, so
    /// command/snapshot behavior is uniform with the live actor.
    #[must_use]
    pub fn over_fake(fake: FakePlayer) -> Self {
        let port: Arc<dyn PlayerPort> = Arc::new(fake);
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        Self { coordinator, port }
    }
}

/// Map the actor's [`PlayerSnapshot`] (transport state) plus the coordinator's
/// queue view into the UI shape.
///
/// The split matters. The actor knows *how* playback is going (state, position,
/// duration, volume, mute) but nothing about the queue: its commands carry a
/// `SongId`, never a `QueueEntryId`, and it publishes `queue_len: 0` by design.
/// The coordinator owns *what* is playing (the queue, the current entry, the
/// mode). Every queue-derived field of the UI snapshot is therefore read from
/// [`CoordinatorView`] — reading them off the actor's snapshot is what once left
/// the player bar blank (empty 当前播放区 + dead mode button) while a song was
/// audibly playing.
#[must_use]
pub fn map_snapshot(
    raw: &PlayerSnapshot,
    view: &CoordinatorView,
    metadata: &std::collections::HashMap<SongId, QueueEntryMeta>,
) -> UiPlayerSnapshot {
    let current = view.current.as_ref();
    let current_id = current.map(|e| e.id);
    let (current_song_id, current_title) = match current.map(|e| &e.item) {
        Some(QueueItem::Library(id)) => {
            // A library song's display title is its resolved metadata — the
            // player bar shows the real title, not a placeholder.
            let title = metadata.get(id).and_then(|meta| meta.title.clone());
            (Some(id.to_string()), title)
        }
        Some(QueueItem::Temporary(t)) => (None, Some(t.display_name.clone())),
        None => (None, None),
    };
    // A session-only temporary item (no library `song_id`) can be imported into
    // the active library (task 11.7); a library entry cannot.
    let (current_can_import, queue) = {
        let current_is_temporary =
            matches!(current.map(|e| &e.item), Some(QueueItem::Temporary(_)));
        let queue = view
            .entries
            .iter()
            .map(|e| {
                let is_temporary = matches!(&e.item, QueueItem::Temporary(_));
                let song_meta = e
                    .item
                    .song_id()
                    .and_then(|id| metadata.get(&id))
                    .cloned()
                    .unwrap_or_default();
                UiQueueEntry {
                    entry_id: e.id.to_string(),
                    song_id: e.item.song_id().map(|id| id.to_string()),
                    title: match &e.item {
                        QueueItem::Library(_) => song_meta.title,
                        QueueItem::Temporary(t) => Some(t.display_name.clone()),
                    },
                    is_current: Some(e.id) == current_id,
                    failed: view.failed_round.contains(&e.id),
                    blocked: view.blocked.contains(&e.id),
                    can_import: is_temporary,
                    artist: song_meta.artist,
                    duration_s: song_meta.duration_s,
                    cover_key: song_meta.cover_key,
                }
            })
            .collect();
        (current_is_temporary, queue)
    };
    UiPlayerSnapshot {
        state: match raw.state {
            PlaybackState::Stopped => "stopped",
            PlaybackState::Loading => "loading",
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
            PlaybackState::Ended => "ended",
            PlaybackState::Failed => "failed",
        },
        position: raw.position,
        duration: raw.duration,
        volume: raw.volume,
        muted: raw.muted,
        current_queue_entry_id: current_id.map(|q| q.to_string()),
        current_song_id,
        queue_len: view.entries.len(),
        mode: match view.mode {
            PlayMode::Sequential => "sequential",
            PlayMode::Shuffle => "shuffle",
            PlayMode::RepeatOne => "repeatOne",
        },
        current_title,
        current_can_import,
        queue,
    }
}

/// A callback the shell supplies to push a UI snapshot to the Tauri frontend
/// (captures `AppHandle` → `app.emit("player://snapshot", ui)`). `Send + Sync`
/// so it can be moved into / shared across the forwarder thread.
pub type SnapshotEmitter = Box<dyn Fn(UiPlayerSnapshot) + Send + Sync>;

/// The production [`PlaybackRecorder`] sink: Core's idempotent
/// `record_playback` (the `recorded_play_sessions` table + `play_count`
/// increment). Storage errors are surfaced as the port's `Err(String)` so the
/// accumulator can log them without ever stopping playback.
pub struct CorePlaybackRecorder {
    database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>,
}

impl CorePlaybackRecorder {
    /// A sink bound to the shared SQLite database.
    #[must_use]
    pub fn new(database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl crate::player::recording::PlaybackRecorder for CorePlaybackRecorder {
    fn record(
        &self,
        session: echo_core::domain::ids::PlaybackSessionId,
        song: echo_core::domain::ids::SongId,
    ) -> Result<bool, String> {
        self.database
            .record_playback(session, song)
            .map_err(|error| error.to_string())
    }
}

/// Spawn the playback-statistics watcher (task 8.10): the missing production
/// half of the accumulator.
///
/// `PlaybackStatsRecorder` had no production caller — every state transition
/// went to the UI and the auto-advance watcher only, so `play_count` never
/// moved and 最近播放 stayed empty. This thread subscribes to the actor's
/// snapshot stream (subscribers fan out) and feeds the accumulator:
///
/// - a changed coordinator `active_load_session` begins a fresh listen session
///   (`begin_library` for library entries; `begin_temporary` never records);
/// - each snapshot updates the known duration and the transport state, then
///   ticks the monotonic clock so mid-play listens still fire exactly once.
///
/// All accumulator state lives on this thread — no locking beyond the
/// coordinator reads it already shares with the forwarder/auto-advance pair.
pub fn spawn_stats_recorder(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    sink: Arc<dyn crate::player::recording::PlaybackRecorder>,
) {
    use crate::player::queue::QueueItem;
    use crate::player::recording::PlaybackStatsRecorder;
    use echo_core::domain::ids::PlaybackSessionId;

    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-stats-recorder".into())
        .spawn(move || {
            let mut recorder = PlaybackStatsRecorder::new(sink);
            let mut last_active: Option<(echo_core::domain::ids::QueueEntryId, PlaybackSessionId)> =
                None;
            while let Ok(snap) = rx.recv() {
                // Detect a fresh load session: the coordinator issues a new
                // PlaybackSessionId on every load_entry. Read the session and
                // the current entry under one lock so they describe the same
                // load.
                let active = coordinator.lock().ok().map(|coord| {
                    (
                        coord.active_load_session(),
                        coord.current().map(|e| e.item.clone()),
                    )
                });
                if let Some((session, item)) = active {
                    if session != last_active {
                        match (session, item) {
                            (Some((_, session_id)), Some(QueueItem::Library(song))) => {
                                recorder.begin_library(session_id, song);
                            }
                            (Some((_, session_id)), _) => {
                                // Temporary (or a session with no current
                                // entry): never recorded (spec: 临时播放项不计统计).
                                recorder.begin_temporary(session_id);
                            }
                            (None, _) => {}
                        }
                        last_active = session;
                    }
                }
                if let Some(duration) = snap.duration {
                    recorder.set_duration(duration);
                }
                recorder.on_state(snap.state);
                recorder.tick();
            }
        })
        .expect("spawn stats recorder thread");
}

/// The Core-delete boundary for [`DeletionCoordinator`]: the closure performs
/// the real delete and returns the undo-operation id, which is captured into a
/// shared slot the caller reads after a commit (a commit implies the id exists).
struct CoreDelete<
    F: Fn(echo_core::domain::ids::SongId) -> Result<String, echo_core::error::Error> + Send + Sync,
> {
    delete_core: F,
    operation: Arc<Mutex<Option<String>>>,
}

impl<
        F: Fn(echo_core::domain::ids::SongId) -> Result<String, echo_core::error::Error> + Send + Sync,
    > crate::player::deletion::DeleteExecutor for CoreDelete<F>
{
    fn hide_for_delete(&self, song: echo_core::domain::ids::SongId) -> Result<(), String> {
        match (self.delete_core)(song) {
            Ok(operation) => {
                if let Ok(mut slot) = self.operation.lock() {
                    *slot = Some(operation);
                }
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Wait for the actor to confirm the current file is released after the
/// coordinated delete's `Stop` (task 8.11 unload barrier). Checks the live
/// snapshot first (the Stop may already have been processed), then the stream,
/// so a snapshot published between `send` and `subscribe` cannot be missed.
fn wait_unload(
    port: &Arc<dyn PlayerPort>,
    timeout: std::time::Duration,
) -> crate::player::deletion::UnloadOutcome {
    use crate::player::deletion::UnloadOutcome;
    let released = |state: PlaybackState| {
        matches!(
            state,
            PlaybackState::Stopped | PlaybackState::Ended | PlaybackState::Failed
        )
    };
    if released(port.snapshot().state) {
        return UnloadOutcome::Confirmed;
    }
    let rx = port.subscribe_snapshots();
    let deadline = std::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(snap) if released(snap.state) => return UnloadOutcome::Confirmed,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    UnloadOutcome::TimedOut
}

/// Delete one song through the [`DeletionCoordinator`] (task 8.11) instead of
/// calling Core directly (which used to bypass the player entirely: deleting
/// the currently-playing song kept the queue showing a track that no longer
/// exists, with no unload barrier before the file operation).
///
/// The whole coordination happens under the coordinator lock the command layer
/// already uses, so no command can interleave with the snapshot/commit/rollback.
/// `delete_core` runs the real delete (Core `DeleteSongs`) and returns the
/// undo-operation id.
///
/// # Errors
///
/// A rolled-back delete (unload timeout / Core refusal) surfaces as `Err` with
/// the reason — never a fabricated success.
pub fn delete_song_coordinated(
    coordinator: &Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    song: echo_core::domain::ids::SongId,
    delete_core: impl Fn(echo_core::domain::ids::SongId) -> Result<String, echo_core::error::Error>
        + Send
        + Sync,
    unload_timeout: std::time::Duration,
) -> Result<String, String> {
    use crate::player::deletion::{DeleteCommit, DeletionCoordinator};

    // The player port outlives the guard (used inside the unload closure).
    let port = {
        let coord = coordinator
            .lock()
            .map_err(|_| "player coordinator poisoned")?;
        coord.player().clone()
    };
    let operation: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let executor = CoreDelete {
        delete_core,
        operation: Arc::clone(&operation),
    };
    let mut deletion = DeletionCoordinator::new(port.clone(), executor);
    let mut coord = coordinator
        .lock()
        .map_err(|_| "player coordinator poisoned")?;
    let mode = coord.mode();
    match deletion.delete_song(coord.queue_mut(), mode, song, || {
        wait_unload(&port, unload_timeout)
    }) {
        Ok(DeleteCommit::Committed) => Ok(operation
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_default()),
        Ok(DeleteCommit::RolledBack) => {
            Err("delete rolled back: the player still holds the file or Core refused".into())
        }
        Err(error) => Err(error),
    }
}

/// The coordinator-owned half of a UI snapshot: the full queue entries, the
/// per-round failed set, the current entry, and the active play mode.
///
/// Passed as one value (rather than four positional arguments) because every
/// field is read from the same lock — the UI shape must be assembled from one
/// consistent view of the queue, never from a mix of sources.
#[derive(Clone, Debug)]
pub struct CoordinatorView {
    /// Every queue entry (current + pending, in play order).
    pub entries: Vec<crate::player::queue::QueueEntry>,
    /// Entries that failed to load/decode in the current round (task 8.7).
    pub failed_round: std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    /// Restored entries preserved as currently unavailable.
    pub blocked: std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    /// The current entry, if any.
    pub current: Option<crate::player::queue::QueueEntry>,
    /// The active playback mode (顺序 / 随机 / 单曲循环).
    pub mode: PlayMode,
}

/// Bridge a sealed snapshot stream to the UI snapshot path.
///
/// The emitter is moved into a background thread that drains the actor's
/// bounded snapshot subscription and forwards each mapped snapshot. A slow
/// consumer has stale snapshots dropped (the actor's receiver is bounded) so
/// this thread never blocks the actor.
///
/// `queue_provider` returns the coordinator's current view, which supplies
/// every queue-derived field of the UI snapshot (task 11.2) — the actor knows
/// nothing about queue membership. `metadata` resolves the presentation
/// metadata (title/artist/duration/cover) of the queue's library entries,
/// batch-cached so repeated position snapshots never re-query per row (task
/// 2.2).
pub fn spawn_forwarder(
    port: Arc<dyn PlayerPort>,
    queue_provider: Arc<dyn Fn() -> CoordinatorView + Send + Sync>,
    metadata: Arc<QueueMetadataResolver>,
    emit: SnapshotEmitter,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-snapshot-forwarder".into())
        .spawn(move || {
            while let Ok(snap) = rx.recv() {
                let view = queue_provider();
                let meta = metadata.resolve(&view.entries);
                emit(map_snapshot(&snap, &view, &meta));
            }
        })
        .expect("spawn snapshot forwarder thread");
}

/// Spawn the session-saver thread (task 8.9 落盘接线): persist the durable
/// playback session — queue, current, history, shuffle bag, mode, volume,
/// mute, last position and the playing source (哪个歌单) — to the atomic
/// desktop-state store.
///
/// Write policy (throttled, never hot):
/// - every **state transition** saves once (a pause/stop/track change is the
///   moment that matters for a crash);
/// - a **volume / mute** change saves once its burst settles (see
///   `AUDIO_SETTLE`), *without* waiting for `min_interval`;
/// - while `Playing`, at most one save per `min_interval` (position progress
///   does not justify an fsync per 10 Hz snapshot).
///
/// The audio case is why the loop wakes on a timeout instead of blocking on the
/// mailbox: 暂停后调音量 (or 静音) leaves *no* follow-up snapshot, so a policy of
/// "save on the next snapshot, throttled" never wrote it — the stored session
/// kept the pre-adjustment volume / `muted:true` and replayed that stale value
/// on the next start.
///
/// A save failure is swallowed (logged only) — persistence must never take
/// playback down with it.
pub fn spawn_session_saver(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    persistence: Arc<dyn SessionPersistence>,
    source: Arc<std::sync::Mutex<Option<String>>>,
    min_interval: std::time::Duration,
) {
    /// How long a volume/mute burst must stay quiet before it is written.
    /// Dragging the slider publishes ~10 snapshots per second; waiting out that
    /// gap collapses the whole drag into one save, while the value the user let
    /// go on is still the one that lands.
    const AUDIO_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
    /// The save loop's wake-up period — short enough that a settled audio
    /// change is flushed promptly after the last snapshot.
    const SAVER_TICK: std::time::Duration = std::time::Duration::from_millis(100);

    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-session-saver".into())
        .spawn(move || {
            let mut last_state = PlaybackState::Stopped;
            let mut last_audio: Option<(f64, bool)> = None;
            let mut last_save = std::time::Instant::now() - min_interval;
            // An audio change observed but not yet written, waiting out its
            // settle window. It carries the snapshot to write, because by the
            // time the window closes no further snapshot has arrived.
            let mut pending_audio: Option<(PlayerSnapshot, std::time::Instant)> = None;

            let save = |snap: &PlayerSnapshot| {
                let session = {
                    let Ok(coord) = coordinator.lock() else {
                        return;
                    };
                    snapshot_queue(
                        coord.queue(),
                        coord.mode(),
                        snap.volume,
                        snap.muted,
                        snap.position,
                        source.lock().ok().and_then(|s| s.clone()).as_deref(),
                    )
                };
                let _ = persistence.save(Some(&session));
            };

            loop {
                match rx.recv_timeout(SAVER_TICK) {
                    Ok(raw) => {
                        let state_changed = raw.state != last_state;
                        let audio_changed = last_audio.is_some_and(|(volume, muted)| {
                            (raw.volume - volume).abs() > VOLUME_EPSILON || raw.muted != muted
                        });
                        last_state = raw.state;
                        last_audio = Some((raw.volume, raw.muted));

                        if audio_changed {
                            pending_audio = Some((raw.clone(), std::time::Instant::now()));
                        }
                        if state_changed {
                            // A transition (暂停 / 停止 / 换曲) is the moment a
                            // crash would lose the most: write at once. It also
                            // supersedes any audio change still settling.
                            save(&raw);
                            last_save = std::time::Instant::now();
                            pending_audio = None;
                            continue;
                        }
                        if raw.state == PlaybackState::Playing
                            && last_save.elapsed() >= min_interval
                        {
                            save(&raw);
                            last_save = std::time::Instant::now();
                            pending_audio = None;
                            continue;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // The player is gone (app quitting): flush a settled
                        // audio change so the last thing the user did survives.
                        if let Some((snap, _)) = &pending_audio {
                            save(snap);
                        }
                        break;
                    }
                }

                if let Some((snap, changed_at)) = &pending_audio {
                    if changed_at.elapsed() >= AUDIO_SETTLE {
                        save(snap);
                        last_save = std::time::Instant::now();
                        pending_audio = None;
                    }
                }
            }
        })
        .expect("spawn session saver thread");
}

/// The restore-or-prime cold-start decision (task 8.9 + 默认态设计):
///
/// 1. A persisted session with entries is restored: the queue is rebuilt,
///    mode/volume/mute re-applied, and the current entry loaded **paused**.
/// 2. With nothing to restore but the library has songs, the *default* kicks
///    in: the first song of 全部歌曲 (as given by `default_view_songs`) is
///    primed into the 播放控制栏 paused, in 列表循环.
/// 3. An empty library yields an empty queue — the bar stays empty.
///
/// `default_view_songs` is called at most once, only in case 2.
pub fn restore_or_prime_playback(
    coordinator: &Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>,
    persistence: &dyn SessionPersistence,
    restore_verdicts: impl FnOnce(
        &crate::player::session::PlaybackSession,
    ) -> Vec<crate::player::session::RestoreVerdict>,
    default_view_songs: impl FnOnce() -> Vec<echo_core::domain::ids::SongId>,
) -> &'static str {
    let restored = match persistence.load() {
        Ok(Some(session)) if !session.entries.is_empty() => Some(session),
        _ => None,
    };
    if let Some(session) = restored {
        let (queue, _summary) = rebuild_queue(&session, &restore_verdicts(&session));
        let volume = session.volume;
        let muted = session.muted;
        let mode = session.mode;
        let position = session.position;
        if let Ok(mut coord) = coordinator.lock() {
            coord.restore_session(queue, mode, volume, muted, position);
        }
        return "restored";
    }
    // Nothing persisted: 默认全部歌曲第一条入栏 (paused, never a sound).
    let songs = default_view_songs();
    if songs.is_empty() {
        return "empty";
    }
    let ctx = ViewContext {
        songs,
        selected_index: 0,
    };
    if let Ok(mut coord) = coordinator.lock() {
        coord.play_context_paused(&ctx);
    }
    "primed"
}

/// Spawn the auto-advance watcher: the missing half of the playback loop.
///
/// The coordinator already owns the "what plays next" rules
/// ([`PlaybackCoordinator::on_played_to_end`] / `on_load_error`), but nothing
/// in production ever called them — a track reaching natural EOF published an
/// `ended` snapshot to the UI and the queue stalled there until the user
/// pressed next manually. This thread subscribes to the actor's snapshot
/// stream (subscribers fan out, so the forwarder is unaffected) and drives the
/// coordinator on the transitions the queue rules expect:
///
/// - `Stopped/… → Ended` (natural EOF, or a load failure surfaced as `Ended`):
///   advance per the active mode (sequential/shuffle/repeat-one).
/// - `… → Failed` (unknown song, unresolvable file): error-skip past the
///   entry, never retrying it within the round.
///
/// The transition guard (only firing when the *previous* snapshot was in a
/// different state) keeps a duplicate `Ended` publish from double-advancing.
pub fn spawn_auto_advance(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-auto-advance".into())
        .spawn(move || {
            let mut last = PlaybackState::Stopped;
            while let Ok(snap) = rx.recv() {
                let state = snap.state;
                if state != last {
                    match state {
                        PlaybackState::Ended => {
                            if let Ok(mut coord) = coordinator.lock() {
                                coord.on_played_to_end();
                            }
                        }
                        PlaybackState::Failed => {
                            if let Ok(mut coord) = coordinator.lock() {
                                coord.on_load_error();
                            }
                        }
                        _ => {}
                    }
                }
                last = state;
            }
        })
        .expect("spawn auto-advance thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::FakePlayer;
    use crate::player::port::PlayerCommand;
    use crate::player::queue::{QueueEntry, QueueItem, ViewContext};
    use crate::player::recording::PlaybackRecorder;
    use crate::player::session::PlaybackSession;
    use echo_core::domain::ids::{PlaybackSessionId, QueueEntryId, SongId};
    use std::sync::Mutex as StdMutex;

    /// An in-memory [`SessionPersistence`] double.
    struct MemSession(StdMutex<Option<PlaybackSession>>);

    impl MemSession {
        fn empty() -> Self {
            Self(StdMutex::new(None))
        }
        fn with(session: PlaybackSession) -> Self {
            Self(StdMutex::new(Some(session)))
        }
    }

    impl SessionPersistence for MemSession {
        fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String> {
            *self.0.lock().expect("mem session lock") = session.cloned();
            Ok(())
        }
        fn load(&self) -> Result<Option<PlaybackSession>, String> {
            Ok(self.0.lock().expect("mem session lock").clone())
        }
    }

    #[test]
    fn restore_or_prime_restores_a_persisted_session_paused() {
        // 冷启动恢复: a persisted session is rebuilt paused — never a sound.
        let controller = PlayerController::over_fake(FakePlayer::new());
        let s1 = SongId::new();
        let s2 = SongId::new();
        let session = crate::player::session::snapshot_queue(
            &ViewContext {
                songs: vec![s1, s2],
                selected_index: 1,
            }
            .build_queue(),
            PlayMode::Shuffle,
            0.5,
            false,
            Some(3.0),
            Some("playlist:p9"),
        );
        let store = MemSession::with(session);
        let outcome = restore_or_prime_playback(
            &controller.coordinator,
            &store,
            |_| vec![],
            || panic!("default view must not be queried when a session restores"),
        );
        assert_eq!(outcome, "restored");
        let coord = controller.coordinator.lock().expect("lock");
        assert_eq!(coord.snapshot().state, PlaybackState::Paused);
        assert_eq!(
            coord.snapshot().volume,
            0.5,
            "volume is restored from the session"
        );
        assert_eq!(coord.mode(), PlayMode::Shuffle);
    }

    #[test]
    fn restore_or_prime_primes_the_first_song_of_the_default_view() {
        // Nothing persisted + a non-empty library: the first 全部歌曲 entry is
        // primed into the player bar paused, in the default mode.
        let controller = PlayerController::over_fake(FakePlayer::new());
        let store = MemSession::empty();
        let s1 = SongId::new();
        let s2 = SongId::new();
        let outcome =
            restore_or_prime_playback(&controller.coordinator, &store, |_| vec![], || vec![s1, s2]);
        assert_eq!(outcome, "primed");
        let coord = controller.coordinator.lock().expect("lock");
        assert_eq!(coord.snapshot().state, PlaybackState::Paused);
        assert_eq!(coord.mode(), PlayMode::Sequential, "default is 列表循环");
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        assert_eq!(
            coord.queue().len(),
            2,
            "the whole default view is the queue"
        );
    }

    #[test]
    fn restore_or_prime_yields_an_empty_bar_for_an_empty_library() {
        let controller = PlayerController::over_fake(FakePlayer::new());
        let store = MemSession::empty();
        let outcome =
            restore_or_prime_playback(&controller.coordinator, &store, |_| vec![], Vec::new);
        assert_eq!(outcome, "empty");
        let coord = controller.coordinator.lock().expect("lock");
        assert!(coord.queue().is_empty());
        assert_eq!(coord.snapshot().state, PlaybackState::Stopped);
    }

    /// A recording sink that captures calls (for wiring assertions).
    #[derive(Default)]
    struct SpySink(StdMutex<Vec<(PlaybackSessionId, SongId)>>);

    impl PlaybackRecorder for SpySink {
        fn record(&self, session: PlaybackSessionId, song: SongId) -> Result<bool, String> {
            self.0.lock().unwrap().push((session, song));
            Ok(true)
        }
    }

    #[test]
    fn stats_recorder_records_a_qualified_library_listen_exactly_once() {
        // Regression: PlaybackStatsRecorder had no production caller, so
        // play_count never moved and 最近播放 stayed empty. The watcher must
        // feed the real snapshot stream and fire the one RecordPlayback once
        // the real monotonic clock crosses the threshold (no force_accumulate).
        let spy = Arc::new(SpySink::default());
        let sink: Arc<dyn PlaybackRecorder> = spy.clone();
        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        spawn_stats_recorder(port.clone(), coordinator.clone(), sink);

        let song = SongId::new();
        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![song],
                selected_index: 0,
            });
        }
        // duration 0.2s → threshold min(30s, 50%) = 0.1s of *real* listening.
        fake.set_duration(0.2);
        fake.set_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(300));
        fake.set_state(PlaybackState::Paused); // settle point fires the record

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let calls = spy.0.lock().unwrap().clone();
            if calls.len() == 1 {
                assert_eq!(calls[0].1, song);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let calls = spy.0.lock().unwrap().clone();
        panic!("expected exactly one record call, got {calls:?}");
    }

    #[test]
    fn stats_recorder_never_records_temporary_items() {
        // 临时播放项不计统计: a temporary load must not reach the sink even
        // after listening well past the threshold.
        let spy = Arc::new(SpySink::default());
        let sink: Arc<dyn PlaybackRecorder> = spy.clone();
        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        spawn_stats_recorder(port.clone(), coordinator.clone(), sink);

        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_temporary(crate::player::coordinator::TemporaryPlay {
                display_name: "temp".into(),
                path: std::path::PathBuf::from("/tmp/echo-test.mp3"),
                duration: Some(0.2),
                on_active_root: false,
            });
        }
        fake.set_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(300));
        fake.set_state(PlaybackState::Paused);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            spy.0.lock().unwrap().is_empty(),
            "temporary playback must never be recorded"
        );
    }

    #[test]
    fn coordinated_delete_removes_current_song_and_commits() {
        // Regression: delete_song bypassed the DeletionCoordinator entirely,
        // so deleting the currently-playing song left the queue showing a
        // track that no longer exists, with no unload barrier (task 8.11).

        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        let s1 = SongId::new();
        let s2 = SongId::new();
        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![s1, s2],
                selected_index: 0,
            });
        }
        fake.set_state(PlaybackState::Playing);

        let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = deleted.clone();
        let outcome = delete_song_coordinated(
            &coordinator,
            s1,
            move |song| {
                sink.lock().unwrap().push(song);
                Ok("op-1".into())
            },
            std::time::Duration::from_secs(2),
        )
        .expect("delete must commit");
        assert_eq!(outcome, "op-1");
        assert_eq!(deleted.lock().unwrap().as_slice(), &[s1]);

        // The queue no longer references the deleted song, and the next
        // available item aligned.
        let view = coordinator.lock().expect("coordinator lock");
        assert!(
            view.queue()
                .entries()
                .iter()
                .all(|e| e.item.song_id() != Some(s1)),
            "deleted song must leave the queue"
        );
        assert_eq!(
            view.current().and_then(|e| e.item.song_id()),
            Some(s2),
            "next entry becomes current"
        );
        // The player was stopped through the port (unload barrier path ran).
        assert_eq!(fake.snapshot().state, PlaybackState::Stopped);
    }

    #[test]
    fn coordinated_delete_rolls_back_when_core_refuses() {
        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        let s1 = SongId::new();
        let s2 = SongId::new();
        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![s1, s2],
                selected_index: 0,
            });
        }

        let outcome = delete_song_coordinated(
            &coordinator,
            s1,
            |_song| Err(echo_core::error::Error::unavailable("song", "locked")),
            std::time::Duration::from_secs(2),
        );
        assert!(
            outcome.is_err(),
            "a Core refusal must roll back, not fake success"
        );

        // The queue was restored: the target is still current.
        let view = coordinator.lock().expect("coordinator lock");
        assert_eq!(
            view.current().and_then(|e| e.item.song_id()),
            Some(s1),
            "rollback keeps the current entry"
        );
        assert_eq!(view.queue().entries().len(), 2);
    }

    #[test]
    fn auto_advance_loads_the_next_track_when_a_song_naturally_ends() {
        // Regression: a track reaching EOF published `ended` to the UI while
        // the queue stalled — `on_played_to_end` had no production caller, so
        // 唱完一首永远不会自动接下一首. The watcher must turn the Playing →
        // Ended transition into a queue advance through the real snapshot
        // subscription path.
        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        spawn_auto_advance(port.clone(), coordinator.clone());

        let s1 = SongId::new();
        let s2 = SongId::new();
        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![s1, s2],
                selected_index: 0,
            });
        }
        assert_eq!(fake.last_loaded_song(), Some(s1));

        // The track plays out; the actor transitions Playing → Ended.
        fake.set_state(PlaybackState::Playing);
        fake.set_state(PlaybackState::Ended);

        // The watcher runs on its own thread; poll for the advance.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if fake.last_loaded_song() == Some(s2) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("auto-advance never loaded the next song after Ended");
    }

    #[test]
    fn auto_advance_does_not_double_advance_on_a_duplicate_ended_publish() {
        // Two Ended snapshots in a row describe one end; the transition guard
        // must advance exactly once (to s2), not skip straight to a stop.
        let fake = Arc::new(FakePlayer::new());
        let port: Arc<dyn PlayerPort> = fake.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        spawn_auto_advance(port.clone(), coordinator.clone());

        let s1 = SongId::new();
        let s2 = SongId::new();
        {
            let mut coord = coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![s1, s2],
                selected_index: 0,
            });
        }
        fake.set_state(PlaybackState::Ended);
        fake.set_state(PlaybackState::Ended); // duplicate publish of the same end

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if fake.last_loaded_song() == Some(s2) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("auto-advance never loaded the next song after Ended");
    }

    #[test]
    fn map_snapshot_maps_states_and_modes() {
        let raw = PlayerSnapshot {
            state: PlaybackState::Playing,
            position: Some(12.5),
            duration: Some(240.0),
            volume: 0.7,
            muted: false,
            queue_len: 3,
            mode: PlayMode::Shuffle,
        };
        let view = CoordinatorView {
            entries: vec![],
            failed_round: Default::default(),
            blocked: Default::default(),
            current: None,
            mode: PlayMode::Shuffle,
        };
        let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
        assert_eq!(ui.state, "playing");
        assert_eq!(ui.position, Some(12.5));
        assert_eq!(ui.mode, "shuffle");
        assert_eq!(ui.current_song_id, None);
        assert_eq!(ui.current_title, None);
        assert!(ui.queue.is_empty());
    }

    #[test]
    fn map_snapshot_derives_current_song_id_from_library_entry() {
        let song = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song),
        };
        let raw = PlayerSnapshot {
            state: PlaybackState::Paused,
            ..PlayerSnapshot::default()
        };
        let view = CoordinatorView {
            entries: vec![entry.clone()],
            failed_round: Default::default(),
            blocked: Default::default(),
            current: Some(entry.clone()),
            mode: PlayMode::Sequential,
        };
        let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
        assert_eq!(ui.current_song_id, Some(song.to_string()));
        assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
        assert_eq!(ui.current_title, None);
        assert_eq!(ui.queue.len(), 1);
        assert_eq!(ui.queue[0].entry_id, entry.id.to_string());
        assert_eq!(ui.queue[0].song_id, Some(song.to_string()));
        assert!(ui.queue[0].is_current);
        assert!(!ui.queue[0].failed);
    }

    #[test]
    fn ui_queue_identity_comes_from_the_coordinator_not_the_transport_snapshot() {
        // Regression for the blank player bar: the real actor never sets a
        // queue-entry id (its `LoadLibrarySong` carries only a `SongId`), so a UI
        // snapshot that read it from `PlayerSnapshot` produced
        // `currentQueueEntryId: null` — the bar rendered its empty 当前播放区 and
        // disabled transport while the song was audibly playing. Even with a
        // bare transport snapshot (no queue fields at all) and a stale
        // `queue_len`/`mode`, the UI must report the coordinator's truth.
        let song = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song),
        };
        let raw = PlayerSnapshot {
            state: PlaybackState::Playing,
            position: Some(1.0),
            queue_len: 0,               // what the actor publishes
            mode: PlayMode::Sequential, // what the actor publishes
            ..PlayerSnapshot::default()
        };
        let view = CoordinatorView {
            entries: vec![entry.clone()],
            failed_round: Default::default(),
            blocked: Default::default(),
            current: Some(entry.clone()),
            mode: PlayMode::RepeatOne,
        };
        let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
        assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
        assert_eq!(ui.current_song_id, Some(song.to_string()));
        assert_eq!(ui.queue_len, 1, "queue length is the coordinator's queue");
        assert_eq!(ui.mode, "repeatOne", "mode is the coordinator's mode");
    }

    #[test]
    fn map_snapshot_surfaces_temporary_title_without_song_id() {
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Temporary(crate::player::queue::TemporaryItem {
                display_name: "outside.m4a".into(),
                path: "/tmp/outside.m4a".into(),
                duration: None,
                on_active_root: false,
            }),
        };
        let raw = PlayerSnapshot {
            state: PlaybackState::Failed,
            ..PlayerSnapshot::default()
        };
        let view = CoordinatorView {
            entries: vec![entry.clone()],
            failed_round: Default::default(),
            blocked: Default::default(),
            current: Some(entry.clone()),
            mode: PlayMode::Sequential,
        };
        let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
        assert_eq!(ui.current_song_id, None);
        assert_eq!(ui.current_title, Some("outside.m4a".into()));
        assert_eq!(ui.state, "failed");
        assert_eq!(ui.queue[0].title.as_deref(), Some("outside.m4a"));
        assert_eq!(ui.queue[0].song_id, None);
        // A temporary item can be imported into the active library (task 11.7).
        assert!(ui.current_can_import);
        assert!(ui.queue[0].can_import);
    }

    #[test]
    fn map_snapshot_flags_failed_entries_and_distinguishes_current() {
        let s1 = SongId::new();
        let current = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s1),
        };
        let s2 = SongId::new();
        let failed = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s2),
        };
        let entries = vec![current.clone(), failed.clone()];
        let failed_round = std::collections::HashSet::from([failed.id]);
        let raw = PlayerSnapshot {
            state: PlaybackState::Playing,
            ..PlayerSnapshot::default()
        };
        let view = CoordinatorView {
            entries,
            failed_round,
            blocked: Default::default(),
            current: Some(current),
            mode: PlayMode::Sequential,
        };
        let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
        assert!(ui.queue[0].is_current);
        assert!(!ui.queue[0].failed);
        assert!(!ui.queue[1].is_current);
        assert!(ui.queue[1].failed);
        assert_eq!(ui.queue[1].song_id, Some(s2.to_string()));
        // Library entries are never importable-as-temporary (task 11.7).
        assert!(!ui.current_can_import);
        assert!(!ui.queue[0].can_import);
        assert!(!ui.queue[1].can_import);
    }

    #[test]
    fn map_snapshot_surfaces_blocked_queue_entries() {
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(SongId::new()),
        };
        let view = CoordinatorView {
            entries: vec![entry.clone()],
            failed_round: Default::default(),
            blocked: std::collections::HashSet::from([entry.id]),
            current: Some(entry),
            mode: PlayMode::Sequential,
        };
        assert!(
            map_snapshot(
                &PlayerSnapshot::default(),
                &view,
                &std::collections::HashMap::new()
            )
            .queue[0]
                .blocked
        );
    }

    #[test]
    fn forwarder_emits_a_ui_snapshot_that_names_the_playing_song() {
        // The full pipeline the player bar depends on, headless: the coordinator
        // builds the queue and loads → the port publishes a snapshot → the
        // forwarder maps it together with the coordinator's view → the UI
        // snapshot names the current entry. Each earlier link passed its unit
        // tests while the chain as a whole was broken (a dead subscription plus
        // a queue-identity field read from the wrong source), so this asserts
        // the end of the chain rather than its parts.
        use crate::player::fake::FakePlayer;
        use crate::player::queue::ViewContext;
        use std::time::Duration;

        let controller = PlayerController::over_fake(FakePlayer::new());
        let song = SongId::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let coordinator = controller.coordinator.clone();
        let provider: Arc<dyn Fn() -> CoordinatorView + Send + Sync> = Arc::new(move || {
            let coord = coordinator.lock().expect("coordinator lock");
            CoordinatorView {
                entries: coord.queue_view(),
                failed_round: coord.failed_round().collect(),
                blocked: coord
                    .queue_view()
                    .iter()
                    .filter(|entry| coord.queue().is_blocked(entry.id))
                    .map(|entry| entry.id)
                    .collect(),
                current: coord.current().cloned(),
                mode: coord.mode(),
            }
        });
        // A metadata resolver over the empty in-memory database: the queue has
        // no library songs to resolve here, but the forwarder must ask for
        // metadata with every snapshot (task 2.2) and never fail on an empty
        // result.
        let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
        let metadata = Arc::new(QueueMetadataResolver::new(db.clone(), db.clone()));
        spawn_forwarder(
            controller.port.clone(),
            provider,
            metadata,
            Box::new(move |ui| {
                let _ = tx.send(ui);
            }),
        );

        {
            let mut coord = controller.coordinator.lock().expect("coordinator lock");
            coord.play_context(&ViewContext {
                songs: vec![song],
                selected_index: 0,
            });
        }

        let ui = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the forwarder must emit a UI snapshot after a play command");
        assert_eq!(ui.state, "playing");
        assert_eq!(ui.current_song_id, Some(song.to_string()));
        assert!(
            ui.current_queue_entry_id.is_some(),
            "the player bar keys its non-empty 当前播放区 off this id"
        );
        assert_eq!(ui.queue.len(), 1);
        assert!(ui.queue[0].is_current);
    }

    #[test]
    fn controller_over_fake_is_headless_and_runnable() {
        let controller = PlayerController::over_fake(FakePlayer::new());
        // The coordinator holds the port; locking and driving it must not need
        // libmpv.
        let mut coord = controller.coordinator.lock().expect("lock");
        let song = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song),
        };
        coord.enqueue(entry);
        // `enqueue` appends to the queue (not yet current); assert the song is
        // present so the coordinator drove a real command headlessly.
        assert!(
            coord
                .queue()
                .entries()
                .iter()
                .any(|e| e.item.song_id() == Some(song)),
            "enqueued song should appear in the queue entries"
        );
    }

    // ------------------------------------------------------------------
    // 会话落盘：暂停态下的音量 / 静音也必须落盘（用户报的"改了音量没保存"）
    // ------------------------------------------------------------------

    /// A [`SessionPersistence`] double that records every write, so a test can
    /// assert both *what* was stored and *how often* the store was touched.
    struct CountingSession(StdMutex<(usize, Option<PlaybackSession>)>);

    impl CountingSession {
        fn new() -> Self {
            Self(StdMutex::new((0, None)))
        }
        fn saves(&self) -> usize {
            self.0.lock().expect("counting session lock").0
        }
        fn last(&self) -> Option<PlaybackSession> {
            self.0.lock().expect("counting session lock").1.clone()
        }
    }

    impl SessionPersistence for CountingSession {
        fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String> {
            let mut guard = self.0.lock().expect("counting session lock");
            guard.0 += 1;
            guard.1 = session.cloned();
            Ok(())
        }
        fn load(&self) -> Result<Option<PlaybackSession>, String> {
            Ok(self.0.lock().expect("counting session lock").1.clone())
        }
    }

    /// Drive a fake player from `Stopped` to **`Paused` at `volume`** with the
    /// saver already attached, and hand back the port plus the recording store.
    ///
    /// Order matters: the saver subscribes to the *live* stream, so it must be
    /// attached before anything is published. The position throttle is set to
    /// an hour, so once the transport settles the only thing that can make the
    /// saver write is the audio change itself — which is what is under test.
    fn paused_fake_with_saver(volume: f64) -> (Arc<dyn PlayerPort>, Arc<CountingSession>) {
        let controller = PlayerController::over_fake(FakePlayer::new());
        let store = Arc::new(CountingSession::new());
        spawn_session_saver(
            controller.port.clone(),
            controller.coordinator.clone(),
            store.clone(),
            Arc::new(StdMutex::new(None)),
            std::time::Duration::from_secs(3600),
        );
        controller
            .port
            .send(PlayerCommand::SetVolume(volume))
            .expect("volume");
        controller
            .port
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: PlaybackSessionId::new(),
            })
            .expect("load");
        controller.port.send(PlayerCommand::Pause).expect("pause");
        (controller.port, store)
    }

    /// Wait (bounded) until `cond` holds, returning whether it did.
    fn wait_until(mut cond: impl FnMut() -> bool, budget: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cond()
    }

    #[test]
    fn a_volume_change_while_paused_reaches_the_store() {
        // 用户暂停后调音量：暂停态下没有后续快照，位置推进的节流也够不着，
        // 所以旧策略（"下一个快照才写，且受 min_interval 约束"）永远写不进去
        // —— 存下的还是调整前的音量 / `muted:true`，下次启动被原样重放。
        let (port, store) = paused_fake_with_saver(0.5);
        // Two transitions (Playing, then Paused) are each written at once —
        // that part always worked. Wait for both so the assertion below is
        // about the volume change alone.
        assert!(
            wait_until(|| store.saves() >= 2, std::time::Duration::from_secs(3)),
            "the transport transitions must be saved [saves={}]",
            store.saves()
        );
        let after_transition = store.saves();

        port.send(PlayerCommand::SetVolume(0.8))
            .expect("set volume");

        // The write must happen once the burst settles (~250 ms) with no
        // further transition and no position progress to carry it.
        assert!(
            wait_until(
                || store.saves() > after_transition,
                std::time::Duration::from_secs(3)
            ),
            "暂停态下调的音量必须落盘，否则下次启动重放的是旧音量"
        );
        assert_eq!(
            store.last().expect("a session was stored").volume,
            0.8,
            "落盘的必须是最新的音量"
        );
    }

    #[test]
    fn a_slider_drag_is_collapsed_into_one_save_of_its_final_value() {
        // Dragging the volume slider publishes a burst of snapshots. Writing
        // each one would fsync ~10×/second; the settle window collapses the
        // burst into a single save — and it must be the value the user let go
        // on, not one from the middle of the drag.
        let (port, store) = paused_fake_with_saver(0.5);
        assert!(
            wait_until(|| store.saves() >= 2, std::time::Duration::from_secs(3)),
            "the transport transitions must be saved first"
        );

        for step in [0.6, 0.7, 0.75, 0.9] {
            port.send(PlayerCommand::SetVolume(step))
                .expect("drag step");
            // Faster than the settle window: one continuous drag.
            std::thread::sleep(std::time::Duration::from_millis(30));
        }

        assert!(
            wait_until(
                || store.last().map(|s| s.volume) == Some(0.9),
                std::time::Duration::from_secs(3)
            ),
            "落盘的必须是松手时的最终音量 [stored={:?}]",
            store.last().map(|s| s.volume)
        );
    }

    // ------------------------------------------------------------------
    // 队列展示元数据 (task 2.2 / 2.3)
    // ------------------------------------------------------------------

    /// A `MemoryDatabase` seeded with one library song (title/artist/duration)
    /// and an attached cover, so resolver tests have real metadata to resolve.
    fn seeded_db() -> (
        Arc<echo_core::application::testing::memory_database::MemoryDatabase>,
        SongId,
    ) {
        use echo_core::application::ports::{SongRepository, UnitOfWork};
        use echo_core::domain::ids::{LibraryRootId, RelativeMediaPath, Revision};
        let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
        let song_id = SongId::new();
        let mut song = echo_core::domain::entities::Song::new(
            song_id,
            LibraryRootId::new(),
            RelativeMediaPath::new("a.flac").expect("path"),
            Revision::INITIAL,
        );
        song.apply_metadata(
            Some("晴天".to_owned()),
            Some("周杰伦".to_owned()),
            Some("叶惠美".to_owned()),
            Some(std::time::Duration::from_secs(239)),
        );
        db.upsert(&song).expect("seed song");
        db.with_tx(Box::new(move |tx| {
            tx.attach_cover(
                song_id,
                &echo_core::application::ports::CoverAssetRef {
                    content_hash: "hash".to_owned(),
                    mime: "image/png".to_owned(),
                    asset_key: "cv1-seeded".to_owned(),
                },
            )
        }))
        .expect("seed cover");
        (db, song_id)
    }

    #[test]
    fn metadata_resolver_resolves_library_entry_fields() {
        // 队列展示信息: library entries resolve to title / artist / duration /
        // cover key — the real presentation fields the queue panel needs, not
        // a generic "歌曲"/"资料库歌曲" placeholder.
        let (db, song_id) = seeded_db();
        let resolver = QueueMetadataResolver::new(db.clone(), db.clone());
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song_id),
        };
        let meta = resolver.resolve(&[entry]);
        let meta = meta.get(&song_id).expect("the queue song is resolved");
        assert_eq!(meta.title.as_deref(), Some("晴天"));
        assert_eq!(meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(meta.duration_s, Some(239));
        assert_eq!(meta.cover_key.as_deref(), Some("cv1-seeded"));
    }

    #[test]
    fn metadata_resolver_skips_temporary_items_without_querying() {
        // 临时项兜底: session-only temporary items are never queried — they
        // carry their own display name, and the resolver does not attempt a
        // library lookup that could fail for a non-library file.
        let (db, _song_id) = seeded_db();
        let resolver = QueueMetadataResolver::new(db.clone(), db.clone());
        let temp = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Temporary(crate::player::queue::TemporaryItem {
                display_name: "访谈录音.m4a".to_owned(),
                path: std::path::PathBuf::from("/tmp/interview.m4a"),
                duration: None,
                on_active_root: false,
            }),
        };
        let meta = resolver.resolve(&[temp]);
        assert!(
            meta.is_empty(),
            "temporary items resolve to no metadata map entry"
        );
    }

    #[test]
    fn metadata_resolver_caches_resolved_ids_across_snapshots() {
        // 高频位置事件复用已解析的队列 DTO: once resolved, a repeated resolve
        // of the same song id answers from the cache — no re-query, no error
        // — so the 10 Hz position stream never triggers per-row lookups.
        let (db, song_id) = seeded_db();
        // Remove the song after the first resolve: the second resolve must
        // still answer from cache rather than querying an absent id.
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song_id),
        };
        let resolver = QueueMetadataResolver::new(db.clone(), db.clone());
        let first = resolver.resolve(std::slice::from_ref(&entry));
        assert!(
            first.contains_key(&song_id),
            "first resolve queries the library"
        );
        use echo_core::application::ports::UnitOfWork;
        let song_id_copy = song_id;
        db.with_tx(Box::new(move |tx| tx.delete_song(song_id_copy)))
            .expect("delete after first resolve");
        let second = resolver.resolve(&[entry]);
        assert_eq!(
            second.get(&song_id).cloned(),
            first.get(&song_id).cloned(),
            "the cached metadata is reused for an unchanged queue id"
        );
    }

    #[test]
    fn metadata_resolver_gives_explicit_nulls_for_unknown_ids() {
        // A queue entry whose id no longer resolves (deletion race / foreign
        // id) gets explicit nulls rather than failing the snapshot — the entry
        // still renders with defined-but-empty presentation fields.
        let (db, _) = seeded_db();
        let resolver = QueueMetadataResolver::new(db.clone(), db.clone());
        let ghost = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(ghost),
        };
        let meta = resolver.resolve(&[entry]);
        assert_eq!(
            meta.get(&ghost).cloned(),
            Some(QueueEntryMeta::default()),
            "an id that does not resolve maps to explicit null metadata"
        );
    }

    #[test]
    fn map_snapshot_attaches_resolved_metadata_to_each_entry() {
        // The UI snapshot's queue entries carry the resolved metadata — title /
        // artist / duration / cover — so the panel renders real songs, and the
        // current entry's title feeds the player bar.
        let song = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song),
        };
        let raw = PlayerSnapshot {
            state: PlaybackState::Playing,
            ..PlayerSnapshot::default()
        };
        let view = CoordinatorView {
            entries: vec![entry.clone()],
            failed_round: Default::default(),
            blocked: Default::default(),
            current: Some(entry),
            mode: PlayMode::Sequential,
        };
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            song,
            QueueEntryMeta {
                title: Some("晴天".to_owned()),
                artist: Some("周杰伦".to_owned()),
                duration_s: Some(239),
                cover_key: Some("cv1-seeded".to_owned()),
            },
        );
        let ui = map_snapshot(&raw, &view, &metadata);
        assert_eq!(ui.current_title.as_deref(), Some("晴天"));
        let row = &ui.queue[0];
        assert_eq!(row.title.as_deref(), Some("晴天"));
        assert_eq!(row.artist.as_deref(), Some("周杰伦"));
        assert_eq!(row.duration_s, Some(239));
        assert_eq!(row.cover_key.as_deref(), Some("cv1-seeded"));
    }
}
