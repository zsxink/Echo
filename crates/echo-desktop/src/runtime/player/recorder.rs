//! Playback-stats recorder sink + watcher (task 8.10).

use echo_core::application::ports::{
    ControlPlanePort, DeviceIdProvider, LibraryProjectionPort, LibraryRepository, SongRepository,
};
use echo_core::domain::ids::{LibraryRootId, PlaybackSessionId};
use echo_core::domain::library::durability::{PlaybackEvent, StateKind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::PlaybackCoordinator;
use super::PlayerPort;
use super::QueueItem;
use crate::platform::playback_retry::{
    PendingPlayback, PlaybackRetryReason, PlaybackRetryStatus, PlaybackRetryStore,
};
use crate::player::recording::{PlaybackRecorder, PlaybackStatsRecorder};

/// The production [`PlaybackRecorder`] sink: Core saves the private session
/// event, shared device buckets and frozen candidate in one directory commit,
/// then catches SQLite up in commit order. The session remains idempotent after
/// a lost response or rebuilt database. Storage errors reach the accumulator
/// without ever stopping playback.
pub struct CorePlaybackRecorder {
    activity: Arc<echo_core::application::local_activity::LocalActivity>,
    database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>,
    /// The portable control surface (`echo/records/play-stats/`).
    control: Arc<dyn ControlPlanePort>,
    /// This device's stable identity — the per-device bucket the count lands
    /// in, so two devices merge additively instead of overwriting each other.
    device_id: Arc<dyn DeviceIdProvider>,
    /// Resolves the active root the record is written under.
    roots: Arc<dyn LibraryRepository>,
    retries: PlaybackRetryStore,
    next_retry: Mutex<Instant>,
    retry_after: Mutex<std::collections::HashMap<PlaybackSessionId, Instant>>,
}

impl CorePlaybackRecorder {
    /// A sink bound to the shared `SQLite` database and the portable control
    /// surface of the active library.
    #[must_use]
    pub fn new(
        database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>,
        control: Arc<dyn ControlPlanePort>,
        device_id: Arc<dyn DeviceIdProvider>,
        roots: Arc<dyn LibraryRepository>,
        activity: Arc<echo_core::application::local_activity::LocalActivity>,
    ) -> Self {
        let retries = PlaybackRetryStore::beside_database(database.path());
        Self {
            activity,
            database,
            control,
            device_id,
            roots,
            retries,
            next_retry: Mutex::new(Instant::now()),
            retry_after: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// The app-private request store for the existing diagnostics surface.
    #[must_use]
    pub fn retry_store(&self) -> PlaybackRetryStore {
        self.retries.clone()
    }

    /// # Errors
    /// Reports protected or unreadable app-local intent bytes without discarding them.
    pub fn retry_status(&self, root: LibraryRootId) -> Result<Vec<PlaybackRetryStatus>, String> {
        self.retries.status(root)
    }

    #[allow(clippy::too_many_lines)] // One ordered retry decision per saved intent.
    fn attempt(&self, entry: &PendingPlayback) -> Result<bool, String> {
        let _activity = self.activity.enter().map_err(|error| error.to_string())?;
        let blocked = if self
            .roots
            .active_root()
            .map_err(|error| error.to_string())?
            .is_none_or(|root| root.id() != entry.root)
        {
            Some(PlaybackRetryReason::InactiveLibrary)
        } else if self
            .control
            .read_manifest(entry.root)
            .map_err(|error| error.to_string())?
            .is_none_or(|manifest| manifest.library_id != entry.library)
        {
            Some(PlaybackRetryReason::LibraryChanged)
        } else if self
            .roots
            .replica_enrollment_pending(entry.root)
            .map_err(|error| error.to_string())?
        {
            Some(PlaybackRetryReason::EnrollmentPending)
        } else {
            None
        };
        if let Some(reason) = blocked {
            self.retries.failed(entry, reason)?;
            return Err(
                "qualified playback remains pending its original library and author".into(),
            );
        }
        let directory = self
            .control
            .durability()
            .ok_or_else(|| "playback directory adapter is unavailable".to_owned())?;
        let replay = match directory.recover(entry.root) {
            Ok(replay) => replay,
            Err(error) => {
                self.retries
                    .failed(entry, PlaybackRetryReason::LibraryUnavailable)?;
                return Err(error.to_string());
            }
        };
        let proven = replay.states().any(|state| {
            state.kind == StateKind::PlaybackEvent
                && state.id == entry.session.as_uuid()
                && serde_json::from_value::<PlaybackEvent>(state.payload.clone())
                    .is_ok_and(|event| event.song_uuid == entry.song.as_uuid())
        });
        if proven {
            // A saved event is projected as its original absolute device bucket.
            // A replacement app/device identity must never create another play.
            if let Err(error) = self.database.project_library_commits(entry.root, &replay) {
                self.retries
                    .failed(entry, PlaybackRetryReason::SavedPendingProjection)?;
                return Err(error.to_string());
            }
            self.retries.complete(entry)?;
            return Ok(false);
        }
        let blocked = if entry.author != self.device_id.current_device_id() {
            Some(PlaybackRetryReason::AuthorChanged)
        } else if SongRepository::by_id(self.database.as_ref(), entry.song)
            .map_err(|error| error.to_string())?
            .is_none_or(|song| song.root() != entry.root || !song.availability().is_visible())
        {
            Some(PlaybackRetryReason::MissingSong)
        } else {
            None
        };
        if let Some(reason) = blocked {
            self.retries.failed(entry, reason)?;
            return Err(
                "qualified playback remains pending its original library and author".into(),
            );
        }
        match echo_core::application::playback_recording::record_qualified_playback(
            self.control.as_ref(),
            self.database.as_ref(),
            self.database.as_ref(),
            entry.author,
            entry.session,
            entry.song,
            entry.qualified_at,
        ) {
            Ok(counted) => {
                // Core also returns false for missing/hidden rows. Only a
                // matching committed event proves that the queue can be cleared.
                let replay = directory
                    .recover(entry.root)
                    .map_err(|error| error.to_string())?;
                let proven = replay.states().any(|state| {
                    state.kind == StateKind::PlaybackEvent
                        && state.id == entry.session.as_uuid()
                        && serde_json::from_value::<PlaybackEvent>(state.payload.clone())
                            .is_ok_and(|event| event.song_uuid == entry.song.as_uuid())
                });
                if !proven {
                    self.retries.failed(entry, PlaybackRetryReason::NotSaved)?;
                    return Err("qualified playback has no matching directory event".into());
                }
                self.retries.complete(entry)?;
                Ok(counted)
            }
            Err(error) => {
                let reason = match error.code() {
                    "library_saved_pending_recovery" => PlaybackRetryReason::SavedPendingRecovery,
                    "library_saved_pending_projection" => {
                        PlaybackRetryReason::SavedPendingProjection
                    }
                    _ => PlaybackRetryReason::NotSaved,
                };
                self.retries.failed(entry, reason)?;
                Err(error.to_string())
            }
        }
    }
}

impl PlaybackRecorder for CorePlaybackRecorder {
    fn record(&self, session: PlaybackSessionId, song: super::SongId) -> Result<bool, String> {
        let Some(local_song) = SongRepository::by_id(self.database.as_ref(), song)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        let root = local_song.root();
        let library = self
            .control
            .read_manifest(root)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "playback library identity is unavailable".to_owned())?
            .library_id;
        let entry = PendingPlayback {
            root,
            library,
            author: self.device_id.current_device_id(),
            session,
            song,
            qualified_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |value| {
                    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
                }),
            reason: PlaybackRetryReason::NotSaved,
            attempts: 0,
        };
        // This stores a request, never a count. A crash before directory commit
        // can retry the same author/session after restart.
        self.retries.enqueue(entry.clone())?;
        self.attempt(&entry)
    }

    fn retry_pending(&self) -> Result<(), String> {
        let now = Instant::now();
        let mut next = self
            .next_retry
            .lock()
            .map_err(|_| "playback retry timer failed")?;
        if now < *next {
            return Ok(());
        }
        *next = now + Duration::from_secs(1);
        drop(next);
        let pending = self.retries.pending()?;
        let (ready, mut due) = {
            let due = self
                .retry_after
                .lock()
                .map_err(|_| "playback retry timer failed")?;
            let ready: Vec<_> = pending
                .into_iter()
                .filter(|entry| {
                    due.get(&entry.session)
                        .is_none_or(|deadline| *deadline <= now)
                })
                .take(16)
                .collect();
            (ready, due.clone())
        };
        for entry in ready {
            match self.attempt(&entry) {
                Ok(_) => {
                    due.remove(&entry.session);
                }
                Err(error) => {
                    let delay = 1u64 << entry.attempts.saturating_add(1).min(6);
                    due.insert(entry.session, now + Duration::from_secs(delay));
                    tracing::warn!(operation = %entry.session, %error, "qualified playback remains pending (non-fatal)");
                }
            }
        }
        *self
            .retry_after
            .lock()
            .map_err(|_| "playback retry timer failed")? = due;
        Ok(())
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
            loop {
                let snap = match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(snapshot) => snapshot,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        recorder.retry_pending();
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
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

#[cfg(test)]
mod retry_tests;
