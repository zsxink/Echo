//! Player port, commands, and snapshot (task 8.1).
//!
//! [`PlayerPort`] is the adapter boundary between the playback coordinator and
//! the platform-specific player actor. `echo-core` never sees this trait; it
//! only exposes [`echo_core::domain::state::PlaybackState`] as shared
//! vocabulary.
//!
//! [`PlayerCommand`] messages flow *to* the actor; [`PlayerSnapshot`] values
//! flow *back* to the coordinator. The real implementation wraps a libmpv
//! handle; [`crate::player::FakePlayer`] is the test double used by
//! coordinator, queue, statistics and platform-control tests — none of which
//! need to load libmpv.

use echo_core::domain::ids::{PlaybackSessionId, QueueEntryId, SongId};
use echo_core::domain::state::PlaybackState;

// ---------------------------------------------------------------------------
// PlayerCommand
// ---------------------------------------------------------------------------

/// Commands sent from the coordinator to the player actor.
///
/// Every variant is a *request*; success/failure is communicated through the
/// next [`PlayerSnapshot`] (state change or error information) or through a
/// dedicated error channel on the coordinator side.
///
/// The actor processes commands sequentially on its dedicated thread — no
/// locking is required on the command path.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerCommand {
    /// Load and begin playing a library song. The actor resolves the
    /// `SongId` to an absolute path via the repository (not the coordinator)
    /// and feeds it to the underlying player backend.
    LoadLibrarySong {
        song_id: SongId,
        session_id: PlaybackSessionId,
    },
    /// Load and play a temporary file (outside the library). The path is
    /// validated by the platform trusted boundary; the actor must only accept
    /// paths it has already verified.
    LoadTemporary {
        /// Display name for metadata; never a filesystem path.
        display_name: String,
        /// Absolute filesystem path, already validated by the platform layer.
        path: std::path::PathBuf,
        session_id: PlaybackSessionId,
    },
    /// Resume playback (transition Paused → Playing).
    Play,
    /// Pause playback (transition Playing → Paused).
    Pause,
    /// Toggle between play and pause.
    TogglePlayPause,
    /// Skip to the next track in the queue. The coordinator is responsible
    /// for advancing the queue and issuing a new `LoadLibrarySong` or
    /// `LoadTemporary`; this command only stops the current item and signals
    /// the coordinator to proceed.
    Next,
    /// Skip to the previous track in the queue. Same coordination contract
    /// as `Next`.
    Previous,
    /// Seek to an absolute position in seconds within the current track.
    Seek(f64),
    /// Set the output volume (0.0 – 1.0, clamped by the actor).
    SetVolume(f64),
    /// Toggle mute, remembering the last non-zero volume.
    ToggleMute,
    /// Notify the actor whether the app is in the foreground. Drives the
    /// snapshot throttle: 10 Hz foreground, 1 Hz background (task 8.4).
    SetForeground(bool),
    /// Hard-stop the player and release resources. The actor transitions to
    /// `Stopped` and signals `unloaded(generation)` so the coordinator can
    /// proceed with root switches or deletions.
    Stop,
    /// Graceful shutdown: stop playback, flush session, destroy the backend.
    /// No further commands are accepted after this.
    Shutdown,
}

// ---------------------------------------------------------------------------
// PlayerSnapshot
// ---------------------------------------------------------------------------

/// Immutable snapshot of the player's current state, published to the
/// coordinator and UI after every meaningful state change.
///
/// The coordinator uses this as its single source of truth for what the
/// player is doing; the UI renders directly from snapshots received via
/// IPC events.
///
/// Fields mirror the player domain vocabulary:
/// - [`PlaybackState`] — the authoritative state machine position.
/// - `position` / `duration` — progress in seconds; `None` when not loaded.
/// - `volume` / `muted` — audio output controls.
/// - `current_item` — the queue entry currently loaded (if any).
/// - `queue_len` — total entries including current; "pending" count for the UI.
/// - `mode` — sequential / shuffle / repeat-one.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    pub state: PlaybackState,
    /// Current playback position in seconds (`None` when not loaded).
    pub position: Option<f64>,
    /// Total duration in seconds (`None` when not loaded or unavailable).
    pub duration: Option<f64>,
    /// Output volume in 0.0–1.0.
    pub volume: f64,
    /// Whether the output is muted.
    pub muted: bool,
    /// The queue entry currently loaded, if any.
    pub current_item: Option<QueueEntryId>,
    /// Total queue length (including current entry).
    pub queue_len: usize,
    /// Active playback mode.
    pub mode: PlayMode,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            state: PlaybackState::default(),
            position: None,
            duration: None,
            volume: 1.0,
            muted: false,
            current_item: None,
            queue_len: 0,
            mode: PlayMode::Sequential,
        }
    }
}

/// Playback mode, matching the spec's three-mode model (顺序 / 随机 / 单曲循环).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    /// Play entries in order; after the last entry, stop.
    #[default]
    Sequential,
    /// Random order; each entry plays once per round before repeating.
    Shuffle,
    /// Repeat the current entry indefinitely (skip on error, never on
    /// explicit next/previous).
    RepeatOne,
}

// ---------------------------------------------------------------------------
// PlayerPort
// ---------------------------------------------------------------------------

/// The adapter boundary between the playback coordinator and the platform
/// player actor. All methods are synchronous to match the existing port
/// convention (no async runtime in `echo-core` or `echo-desktop` today).
///
/// The *real* implementation wraps a bounded channel to the libmpv actor
/// thread. [`crate::player::FakePlayer`] is the in-process test double
/// that processes commands immediately — it proves the coordinator, queue,
/// statistics and platform-control logic works without loading libmpv.
pub trait PlayerPort: Send + Sync {
    /// Send a command to the player actor.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError::ActorClosed`] if the actor channel is closed
    /// (actor shut down or crashed). Returns [`PlayerError::Backend`] if the
    /// command was accepted but the backend rejected it.
    fn send(&self, cmd: PlayerCommand) -> Result<(), PlayerError>;

    /// The current snapshot. The coordinator can poll this at any time; the
    /// actor also pushes snapshots after every meaningful state change.
    #[must_use]
    fn snapshot(&self) -> PlayerSnapshot;

    /// Subscribe to snapshot updates. Each subscriber receives a new
    /// `PlayerSnapshot` every time the actor publishes a change (throttled
    /// by the actor: 10 Hz foreground, 1 Hz background). The receiver is
    /// bounded; a slow consumer will have stale snapshots dropped, never
    /// block the actor.
    #[must_use]
    fn subscribe_snapshots(&self) -> std::sync::mpsc::Receiver<PlayerSnapshot>;
}

// ---------------------------------------------------------------------------
// PlayerError
// ---------------------------------------------------------------------------

/// Errors from the player adapter boundary. Distinguished from domain errors
/// because they relate to the platform backend, not business rules.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// The actor channel is closed (actor shut down or crashed).
    #[error("player actor channel closed")]
    ActorClosed,
    /// The command was sent but the backend rejected it (file not found,
    /// decode error, etc.). The error message is human-readable and safe to
    /// log; it must not contain absolute paths.
    #[error("player backend error: {message}")]
    Backend { message: String },
}
