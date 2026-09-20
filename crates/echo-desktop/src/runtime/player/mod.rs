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

use crate::player::actor::{FfiSpawnError, PlayerActor, SongResolver};
use crate::player::coordinator::PlaybackCoordinator;
use crate::player::port::{PlayMode, PlayerPort, PlayerSnapshot, VOLUME_EPSILON};
use crate::player::queue::{QueueItem, ViewContext};
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
    /// The entry's opaque `cover://` asset key (design §16); `None` when the
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
pub use snapshot::{map_snapshot, spawn_forwarder};

#[cfg(test)]
mod tests;
