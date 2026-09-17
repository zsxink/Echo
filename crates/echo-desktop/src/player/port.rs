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

use echo_core::domain::ids::{PlaybackSessionId, SongId};
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
    /// Load a library song **without starting playback** — the file is decoded
    /// (so `duration` and the embedded cover are real) and held at position 0
    /// in `Paused`. Used by the cold-start restore / default-current primed
    /// state, which must show the real track in 播放控制栏 while never making a
    /// sound before the user presses play (设计: 不得自动开始发声).
    LoadLibrarySongPaused {
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
    /// Publish the user-selected queue traversal mode with the next player
    /// snapshot. The coordinator remains the owner of queue behavior; the
    /// actor carries this value solely so a mode-only change reaches snapshot
    /// subscribers even when no media property has changed.
    SetMode(PlayMode),
    /// Set the output volume (0.0 – 1.0, clamped by the actor).
    SetVolume(f64),
    /// Toggle mute, remembering the last non-zero volume.
    ///
    /// This is the *user intent* command (播放控制栏按钮、媒体键): "switch
    /// whatever the output is doing". It is deliberately relative.
    ToggleMute,
    /// Set mute to an **absolute** state, idempotently — the counterpart of
    /// [`Self::ToggleMute`] for callers that already know the value they want.
    ///
    /// Cold-start session restore is exactly such a caller: the persisted
    /// session records the mute state the user left behind, and restoring has
    /// to *reproduce* it. A relative toggle cannot express that — replaying the
    /// same restore (a double-invoked command, React `StrictMode` mounting twice)
    /// flips the flag straight back and silently un-mutes the player. Applying
    /// an absolute value lands on the recorded state no matter how many times
    /// it runs.
    SetMute(bool),
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

/// Tolerance for comparing two volume values.
///
/// A volume makes a round trip through mpv (`0.0–1.0` here, `0.0–100.0` in
/// mpv) before it is observed back, so the same loudness can come back a few
/// ULPs off. Comparing exactly would make the actor (and the session saver)
/// treat a no-op as a change and publish / fsync on every snapshot tick.
pub const VOLUME_EPSILON: f64 = 1e-6;

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
/// - `queue_len` — total entries including current; "pending" count for the UI.
/// - `mode` — sequential / shuffle / repeat-one.
///
/// This snapshot deliberately carries **transport state only**. Queue membership,
/// the current entry's identity and the active mode are owned by the
/// `PlaybackCoordinator` (the actor never receives a `QueueEntryId`, and
/// `build_snapshot` publishes `queue_len: 0` for exactly that reason). The UI
/// snapshot is therefore assembled from both sources — see
/// `runtime::player::map_snapshot` — and must never read queue identity off this
/// struct.
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
            queue_len: 0,
            mode: PlayMode::Sequential,
        }
    }
}

/// Playback mode, matching the spec's three-mode model (列表循环 / 随机 / 单曲循环).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    /// 列表循环: play entries in order and, after the last entry, wrap around
    /// to the first one again. This is the **default** mode — a library is
    /// meant to keep playing, not to stop after its last song.
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

/// A shared, owned player port — lets the composition root pass `Arc<dyn
/// PlayerPort>` where a concrete owned player is expected (e.g. the
/// coordinator's generic `P: PlayerPort`).
impl PlayerPort for std::sync::Arc<dyn PlayerPort> {
    fn send(&self, cmd: PlayerCommand) -> Result<(), PlayerError> {
        (**self).send(cmd)
    }

    fn snapshot(&self) -> PlayerSnapshot {
        (**self).snapshot()
    }

    fn subscribe_snapshots(&self) -> std::sync::mpsc::Receiver<PlayerSnapshot> {
        (**self).subscribe_snapshots()
    }
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
