//! Playback-stats recorder sink + watcher (task 8.10).

use std::sync::{Arc, Mutex};

use echo_core::domain::ids::PlaybackSessionId;

use super::PlaybackCoordinator;
use super::PlayerPort;
use super::QueueItem;
use crate::player::recording::{PlaybackRecorder, PlaybackStatsRecorder};

/// The production [`PlaybackRecorder`] sink: Core's idempotent
/// `record_playback` (the `recorded_play_sessions` table + `play_count`
/// increment). Storage errors are surfaced as the port's `Err(String)` so the
/// accumulator can log them without ever stopping playback.
pub struct CorePlaybackRecorder {
    database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>,
}

impl CorePlaybackRecorder {
    /// A sink bound to the shared `SQLite` database.
    #[must_use]
    pub const fn new(database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl PlaybackRecorder for CorePlaybackRecorder {
    fn record(&self, session: PlaybackSessionId, song: super::SongId) -> Result<bool, String> {
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
///
/// # Panics
///
/// If the `echo-stats-recorder` worker thread cannot be spawned. This happens
/// at composition time, before any playback exists, so it is a fatal startup
/// fault rather than a recoverable runtime error.
#[allow(clippy::needless_pass_by_value)] // The port is moved into the recorder thread.
pub fn spawn_stats_recorder(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    sink: Arc<dyn PlaybackRecorder>,
) {
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
