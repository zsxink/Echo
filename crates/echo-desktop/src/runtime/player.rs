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
use echo_core::domain::state::PlaybackState;
use echo_core::error::Error;

use crate::player::actor::{FfiSpawnError, PlayerActor, SongResolver};
use crate::player::coordinator::PlaybackCoordinator;
use crate::player::fake::FakePlayer;
use crate::player::port::{PlayMode, PlayerPort, PlayerSnapshot};
use crate::player::queue::QueueItem;

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
    pub title: Option<String>,
    pub is_current: bool,
    pub failed: bool,
    /// True when this is a session-only temporary item (no `song_id`) that can
    /// be imported into the active library (task 11.7). Derived; never false
    /// for a library entry.
    pub can_import: bool,
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

/// Map a raw actor [`PlayerSnapshot`] plus the current queue state into the
/// UI shape. `entries` is the coordinator's full queue (current + pending, in
/// play order); `failed_round` is the set of entry ids that failed to load or
/// decode this round (task 8.7); `current` is the current queue entry, which
/// supplies the `currentSongId` / title the snapshot itself lacks.
#[must_use]
pub fn map_snapshot(
    raw: &PlayerSnapshot,
    entries: &[crate::player::queue::QueueEntry],
    failed_round: &std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    current: Option<&crate::player::queue::QueueEntry>,
) -> UiPlayerSnapshot {
    let current_id = current.map(|e| e.id);
    let (current_song_id, current_title) = match current.map(|e| &e.item) {
        Some(QueueItem::Library(id)) => (Some(id.to_string()), None),
        Some(QueueItem::Temporary(t)) => (None, Some(t.display_name.clone())),
        None => (None, None),
    };
    // A session-only temporary item (no library `song_id`) can be imported into
    // the active library (task 11.7); a library entry cannot.
    let (current_can_import, queue) = {
        let current_is_temporary =
            matches!(current.map(|e| &e.item), Some(QueueItem::Temporary(_)));
        let queue = entries
            .iter()
            .map(|e| {
                let is_temporary = matches!(&e.item, QueueItem::Temporary(_));
                UiQueueEntry {
                    entry_id: e.id.to_string(),
                    song_id: e.item.song_id().map(|id| id.to_string()),
                    title: match &e.item {
                        QueueItem::Library(_) => None,
                        QueueItem::Temporary(t) => Some(t.display_name.clone()),
                    },
                    is_current: Some(e.id) == current_id,
                    failed: failed_round.contains(&e.id),
                    can_import: is_temporary,
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
        current_queue_entry_id: raw.current_item.map(|q| q.to_string()),
        current_song_id,
        queue_len: raw.queue_len,
        mode: match raw.mode {
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

/// The coordinator's queue view the forwarder asks for on each snapshot: the
/// full entries, the per-round failed set, and the current entry. Factored as a
/// named tuple so the forwarder closure type stays clippy-clean.
pub type QueueView = (
    Vec<crate::player::queue::QueueEntry>,
    std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    Option<crate::player::queue::QueueEntry>,
);

/// Bridge a sealed snapshot stream to the UI snapshot path.
///
/// The emitter is moved into a background thread that drains the actor's
/// bounded snapshot subscription and forwards each mapped snapshot. A slow
/// consumer has stale snapshots dropped (the actor's receiver is bounded) so
/// this thread never blocks the actor.
///
/// `queue_provider` returns the coordinator's current queue view — the full
/// entries, the per-round failed set, and the current entry — so the UI
/// snapshot can carry the whole queue for the panel (task 11.2), not just the
/// current song.
pub fn spawn_forwarder(
    port: Arc<dyn PlayerPort>,
    queue_provider: Arc<dyn Fn() -> QueueView + Send + Sync>,
    emit: SnapshotEmitter,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-snapshot-forwarder".into())
        .spawn(move || {
            while let Ok(snap) = rx.recv() {
                let (entries, failed, current) = queue_provider();
                emit(map_snapshot(&snap, &entries, &failed, current.as_ref()));
            }
        })
        .expect("spawn snapshot forwarder thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::FakePlayer;
    use crate::player::queue::{QueueEntry, QueueItem};
    use echo_core::domain::ids::{QueueEntryId, SongId};

    #[test]
    fn map_snapshot_maps_states_and_modes() {
        let raw = PlayerSnapshot {
            state: PlaybackState::Playing,
            position: Some(12.5),
            duration: Some(240.0),
            volume: 0.7,
            muted: false,
            current_item: None,
            queue_len: 3,
            mode: PlayMode::Shuffle,
        };
        let ui = map_snapshot(&raw, &[], &Default::default(), None);
        assert_eq!(ui.state, "playing");
        assert_eq!(ui.position, Some(12.5));
        assert_eq!(ui.mode, "shuffle");
        assert_eq!(ui.queue_len, 3);
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
            current_item: Some(entry.id),
            ..PlayerSnapshot::default()
        };
        let ui = map_snapshot(
            &raw,
            std::slice::from_ref(&entry),
            &Default::default(),
            Some(&entry),
        );
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
        let ui = map_snapshot(
            &raw,
            std::slice::from_ref(&entry),
            &Default::default(),
            Some(&entry),
        );
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
            current_item: Some(current.id),
            queue_len: 2,
            ..PlayerSnapshot::default()
        };
        let ui = map_snapshot(&raw, &entries, &failed_round, Some(&current));
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
}
