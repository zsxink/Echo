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
//!
//! The command surface is split into capability submodules: [`metadata`] (the
//! queue metadata resolver), [`snapshot`] (UI mapping + forwarder), [`recorder`]
//! (stats recorder sink), [`coordinated_delete`] (player-aware delete),
//! [`session`] (session saver + cold-start restore) and [`auto_advance`]
//! (auto-advance watcher).

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, RwLock};

use echo_core::application::scan::ScanDeps;
use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;
use echo_core::error::Error;

use crate::player::actor::{FfiSpawnError, LoudnessLookup, PlayerActor, SongResolver};
use crate::player::coordinator::PlaybackCoordinator;
use crate::player::port::{PlayMode, PlayerPort, PlayerSnapshot, VOLUME_EPSILON};
use crate::player::queue::{QueueItem, QueueRevision, ViewContext};
use crate::player::session::SessionPersistence;

#[cfg(test)]
use crate::player::fake::FakePlayer;

/// One entry of the playback queue, as the queue panel renders it (task 11.2).
///
/// It carries the stable `entry_id` (queue identity, distinct from `song_id` so
/// a repeated song appears as independent entries), the `song_id` for library
/// songs or the session-only `title` for temporary items, whether it is the
/// current entry, and whether it failed to load/decode this round (error state).
/// Everything is derived from the authoritative queue — never fabricated.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Serde DTO shape mirrors independent player capabilities for the frontend contract.
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
    /// The entry's opaque `cover://` asset key (docs/DESIGN.md §2.4); `None` when the
    /// song carries no embedded artwork. Never an absolute path.
    pub cover_key: Option<String>,
}

/// Lyrics carried by a session-only temporary playback item.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTemporaryLyrics {
    pub source: Option<String>,
    pub timed: bool,
    pub lines: Vec<UiLyricLine>,
    pub plain_text: String,
    pub parse_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiLyricLine {
    pub seconds: f64,
    pub text: String,
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
    pub album: Option<String>,
    pub duration_s: Option<u64>,
    pub cover_key: Option<String>,
}

/// The UI-facing playback snapshot (mirrors the frontend `UiPlayerSnapshot`).
/// Every value is derived from the authoritative [`PlayerSnapshot`] + the
/// coordinator's queue — never fabricated.
///
/// The payload carries **transport state only**. The queue projection and the
/// effects snapshot travel on their own events (`player://queue`,
/// `player://effects`), so the 10 Hz stream stays small no matter how long the
/// queue is — the payload size is a property of the transport, not of the
/// library.
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
    pub current_artist: Option<String>,
    pub current_album: Option<String>,
    pub current_cover_key: Option<String>,
    pub current_lyrics: Option<UiTemporaryLyrics>,
    /// True when the current entry is a session-only temporary item that can be
    /// imported into the active library (task 11.7).
    pub current_can_import: bool,
    /// The revision of the queue projection the frontend currently holds.
    ///
    /// It travels with every position frame so the frontend can tell that the
    /// queue changed without receiving the whole projection again.
    pub queue_revision: u64,
}

/// The queue projection the queue panel renders (task 11.2).
///
/// It is published on its own event and only when the queue revision changed,
/// so the 10 Hz position stream never carries the whole queue.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiQueueProjection {
    /// The queue revision this projection was built at.
    pub revision: u64,
    /// Every queue entry (current + pending, in play order).
    pub entries: Vec<UiQueueEntry>,
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
    /// Uses Core's root-relative path invariant. The absolute path is consumed
    /// only on the actor thread and never returned to a DTO.
    #[must_use]
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
            root.resolve_song_path(&song)
        })
    }

    /// Build the per-song loudness lookup over the same repository. A missing
    /// song or an untagged row yields `None` (0 dB); storage errors are
    /// returned so the actor can log a diagnostic and degrade safely.
    #[must_use]
    pub fn loudness_lookup(deps: &Arc<ScanDeps>) -> LoudnessLookup {
        let songs = deps.songs.clone();
        Arc::new(move |song_id| Ok(songs.by_id(song_id)?.and_then(|song| song.loudness_tags())))
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
        Self::spawn_mpv_with_loudness(libmpv_path, resolver, None)
    }

    /// Production assembly: the actor also receives the loudness lookup.
    ///
    /// # Errors
    /// Propagates [`FfiSpawnError`] when libmpv or the actor thread is
    /// unavailable.
    pub fn spawn_mpv_with_loudness(
        libmpv_path: &std::path::Path,
        resolver: SongResolver,
        loudness_lookup: Option<LoudnessLookup>,
    ) -> Result<Self, FfiSpawnError> {
        let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
        let actor = PlayerActor::spawn_mpv_with_loudness(
            libmpv_path,
            snapshot,
            Some(resolver),
            loudness_lookup,
        )?;
        let port: Arc<dyn PlayerPort> = Arc::new(actor);
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        Ok(Self { coordinator, port })
    }
}

#[cfg(test)]
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

/// A callback the shell supplies to push a UI snapshot to the Tauri frontend
/// (captures `AppHandle` → `app.emit("player://snapshot", ui)`). `Send + Sync`
/// so it can be moved into / shared across the forwarder thread.
pub type SnapshotEmitter = Box<dyn Fn(UiPlayerSnapshot) + Send + Sync>;

/// A callback the shell supplies to push a queue projection to the frontend
/// (`app.emit("player://queue", projection)`).
pub type QueueEmitter = Box<dyn Fn(UiQueueProjection) + Send + Sync>;

/// A callback the shell supplies to push an effects snapshot to the frontend
/// (`app.emit("player://effects", snapshot)`).
pub type EffectsEmitter = Box<dyn Fn(crate::ipc::effects::EffectsSnapshotDto) + Send + Sync>;

/// The three playback event sinks the shell installs.
///
/// The streams are separate on purpose: the transport snapshot is published at
/// 10 Hz and must stay small, while the queue projection and the effects
/// snapshot are published only when their content actually changed.
pub struct PlayerEmitters {
    /// The 10 Hz transport snapshot (`player://snapshot`).
    pub snapshot: SnapshotEmitter,
    /// The queue projection (`player://queue`).
    pub queue: QueueEmitter,
    /// The effects snapshot (`player://effects`).
    pub effects: EffectsEmitter,
}

/// The transport half of the coordinator's state: what the position stream
/// needs on every frame.
///
/// The queue projection is deliberately **not** part of it — the entries are
/// built only when the queue revision changed (see [`QueueProjection`]).
#[derive(Clone, Debug)]
pub struct CoordinatorView {
    /// The current entry, if any.
    pub current: Option<crate::player::queue::QueueEntry>,
    /// The active playback mode (顺序 / 随机 / 单曲循环).
    pub mode: PlayMode,
    /// The queue revision this view was read at.
    pub revision: QueueRevision,
}

/// The queue projection as the coordinator holds it, before it is mapped into
/// the UI shape.
#[derive(Clone, Debug)]
pub struct QueueProjection {
    /// The revision the projection was built at.
    pub revision: QueueRevision,
    /// Every queue entry (current + pending, in play order).
    pub entries: Vec<crate::player::queue::QueueEntry>,
    /// Entries that failed to load/decode in the current round (task 8.7).
    pub failed_round: std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    /// Restored entries preserved as currently unavailable.
    pub blocked: std::collections::HashSet<echo_core::domain::ids::QueueEntryId>,
    /// The current entry's id, if any.
    pub current: Option<echo_core::domain::ids::QueueEntryId>,
}

/// One frame of playback state as the forwarder sees it.
///
/// The projection is present only when the queue changed since the revision
/// the consumer last received — the provider decides that inside one
/// coordinator lock, so the revision and the entries always agree.
#[derive(Clone, Debug)]
pub struct PlayerFrame {
    /// The transport state of this frame.
    pub view: CoordinatorView,
    /// The queue projection, when it changed.
    pub projection: Option<QueueProjection>,
}

/// Supplies the forwarder with one frame of playback state.
///
/// The provider receives the revision the consumer last received, so it can
/// decide — inside one coordinator lock — whether the queue projection has to
/// be rebuilt at all.
pub type PlayerFrameProvider = Arc<dyn Fn(Option<QueueRevision>) -> PlayerFrame + Send + Sync>;

/// Produces the current effects snapshot (the source of `player://effects`).
pub type EffectsSnapshotProvider =
    Arc<dyn Fn() -> crate::ipc::effects::EffectsSnapshotDto + Send + Sync>;

/// The event carrying the 10 Hz transport snapshot to the frontend.
pub const PLAYER_SNAPSHOT_EVENT: &str = "player://snapshot";

/// The event carrying the queue projection to the frontend.
pub const PLAYER_QUEUE_EVENT: &str = "player://queue";

/// The event carrying the effects snapshot to the frontend.
pub const PLAYER_EFFECTS_EVENT: &str = "player://effects";

mod auto_advance;
mod coordinated_delete;
mod metadata;
mod recorder;
mod session;
mod snapshot;

// Re-export the split items at the `player` module path so the app shell
// (`echo-app`) keeps calling `crate::runtime::player::*` unchanged.
pub use auto_advance::spawn_auto_advance;
pub use coordinated_delete::delete_song_coordinated;
pub use metadata::QueueMetadataResolver;
pub use recorder::{spawn_stats_recorder, CorePlaybackRecorder};
pub use session::{restore_or_prime_playback, spawn_session_saver};
pub use snapshot::{
    map_queue_projection, map_snapshot, queue_projection, spawn_forwarder, ui_queue_projection,
};

#[cfg(test)]
mod tests;
