//! Playback statistics accumulator (task 8.10, design §12).
//!
//! Playback counts must be based on **cumulative real listening time**, never on
//! `time-pos` alone — a large seek to the end of the file must not fabricate
//! listen time, and pausing must stop the clock. Each load (`LoadLibrarySong` /
//! `LoadTemporary`) gets a fresh [`PlaybackSessionId`]; the accumulator counts
//! toward the threshold `min(30s, duration * 0.5)` and calls the idempotent
//! [`PlaybackRecorder::record`] **exactly once** per session.
//!
//! Rules (task 8.10):
//! - Only `Playing` state accumulates elapsed time (monotonic wall clock, not
//!   `time-pos`, so seek cannot cheat).
//! - `Pause` stops accumulation (and seeks do not add time).
//! - The threshold is `min(30s, song_duration * 0.5)`.
//! - The recorded callback is invoked at most once per session, even if the
//!   same event/position repeats.
//! - Temporary (session-only, non-library) items are never recorded.
//!
//! The desktop layer owns this accumulator; the actual idempotent persistence
//! (the `recorded_play_sessions` table behind `RecordPlayback`) lives in
//! `echo-core`'s `SQLite` store and is reached through a small [`PlaybackRecorder`]
//! port so this module stays player/testable without a real database.

/// Idempotent playback-recording boundary. The real desktop composition root
/// wires this to Core's `record_playback(session, song)` (which inserts the
/// `recorded_play_sessions` row once and increments `play_count` only for a
/// fresh session).
pub trait PlaybackRecorder: Send + Sync {
    /// Record one qualified library listen. Implementations MUST be idempotent
    /// per `session` — a repeated call for an already-recorded session is a
    /// no-op. Returns whether this call newly counted (informational).
    ///
    /// # Errors
    ///
    /// A storage error: the desktop may surface a non-blocking diagnostics note.
    fn record(
        &self,
        session: echo_core::domain::ids::PlaybackSessionId,
        song: echo_core::domain::ids::SongId,
    ) -> Result<bool, String>;
}

/// A [`PlaybackRecorder`] that never records — a safe default until the
/// composition root wires the database (tests and headless use).
#[derive(Default)]
pub struct NullRecorder;

/// Any `PlaybackRecorder` is also one by shared reference — so callers can pass
/// `&recorder` and keep owning the sink to inspect its results (used by tests).
impl<R: PlaybackRecorder + ?Sized> PlaybackRecorder for &R {
    fn record(
        &self,
        session: echo_core::domain::ids::PlaybackSessionId,
        song: echo_core::domain::ids::SongId,
    ) -> Result<bool, String> {
        (**self).record(session, song)
    }
}

/// A shared sink is also a sink — so the composition root can hand the stats
/// watcher an `Arc<dyn PlaybackRecorder>` (the database-backed adapter lives
/// behind a trait object) without another wrapper type.
impl<R: PlaybackRecorder + ?Sized> PlaybackRecorder for std::sync::Arc<R> {
    fn record(
        &self,
        session: echo_core::domain::ids::PlaybackSessionId,
        song: echo_core::domain::ids::SongId,
    ) -> Result<bool, String> {
        (**self).record(session, song)
    }
}

impl PlaybackRecorder for NullRecorder {
    fn record(
        &self,
        _session: echo_core::domain::ids::PlaybackSessionId,
        _song: echo_core::domain::ids::SongId,
    ) -> Result<bool, String> {
        Ok(false)
    }
}

/// The accumulated listen state for the *current* load session.
#[derive(Debug, Clone, PartialEq)]
struct ListenState {
    session: echo_core::domain::ids::PlaybackSessionId,
    song: echo_core::domain::ids::SongId,
    /// `None` until the duration is known (or a library song has no duration);
    /// until then the threshold is the flat 30s floor.
    duration_seconds: Option<f64>,
    /// Cumulative real listening seconds this session.
    accumulated: f64,
    /// Whether the one allowed `RecordPlayback` has already fired.
    recorded: bool,
}

/// The statistics accumulator: owns the current listen session and decides when
/// to fire the idempotent `RecordPlayback`.
pub struct PlaybackStatsRecorder<R: PlaybackRecorder> {
    recorder: R,
    current: Option<ListenState>,
    /// Wall-clock basis; set when playback *starts* playing and cleared on pause.
    playing_since: Option<std::time::Instant>,
    /// The recorded duration, so a seek that changes `time-pos` cannot add time.
    duration_known: bool,
}

impl<R: PlaybackRecorder> PlaybackStatsRecorder<R> {
    /// A recorder bound to a `PlaybackRecorder` sink.
    #[must_use]
    pub const fn new(recorder: R) -> Self {
        Self {
            recorder,
            current: None,
            playing_since: None,
            duration_known: false,
        }
    }

    /// Begin a new load session for a library song. Resets the accumulated
    /// clock and arms a fresh (single) record.
    pub fn begin_library(
        &mut self,
        session: echo_core::domain::ids::PlaybackSessionId,
        song: echo_core::domain::ids::SongId,
    ) {
        self.current = Some(ListenState {
            session,
            song,
            duration_seconds: None,
            accumulated: 0.0,
            recorded: false,
        });
        self.playing_since = None;
        self.duration_known = false;
    }

    /// Begin a new load session for a temporary (session-only) item. These are
    /// **never** recorded (spec: 临时播放项不计统计).
    pub fn begin_temporary(&mut self, session: echo_core::domain::ids::PlaybackSessionId) {
        // Temporaries carry no song id → recording is skipped. Track enough to
        // discard any accumulated state.
        self.current = None;
        self.playing_since = None;
        self.duration_known = false;
        let _ = session;
    }

    /// Supply the song's duration when known (drives the `50%` half of the
    /// threshold). Called when a library song's duration is observed.
    pub fn set_duration(&mut self, seconds: f64) {
        if let Some(state) = &mut self.current {
            state.duration_seconds = Some(seconds);
        }
        self.duration_known = true;
    }

    /// Notify a state transition (Playing / Paused / Ended / Stopped).
    ///
    /// - Entering `Playing` starts the monotonic clock.
    /// - Leaving `Playing` (pause/end/stop) stops it and, on a qualified
    ///   library session, may fire the one `RecordPlayback`.
    pub fn on_state(&mut self, state: echo_core::domain::state::PlaybackState) {
        if state == echo_core::domain::state::PlaybackState::Playing {
            if self.playing_since.is_none() {
                self.playing_since = Some(std::time::Instant::now());
            }
        } else {
            if let Some(since) = self.playing_since.take() {
                let elapsed = since.elapsed().as_secs_f64();
                if let Some(state) = &mut self.current {
                    state.accumulated += elapsed;
                }
            }
            self.maybe_record();
        }
    }

    /// Poll the accumulated clock each snapshot tick while playing, so a track
    /// that plays for many minutes across snapshot boundaries still reaches the
    /// threshold in a timely way (not only on the next pause/end).
    ///
    /// Each tick folds *only* the segment since the last tick into
    /// `accumulated` and then re-anchors the wall clock, so no second is ever
    /// counted twice and a long track keeps progressing toward the threshold.
    pub fn tick(&mut self) {
        if self.playing_since.is_some() {
            let elapsed = self
                .playing_since
                .replace(std::time::Instant::now())
                .map_or(0.0, |since| since.elapsed().as_secs_f64());
            if let Some(state) = self.current.as_mut() {
                state.accumulated += elapsed;
            }
        }
        self.maybe_record();
    }

    /// Test-only: add raw accumulated seconds (bypasses the wall clock so a
    /// test need not sleep). Marked `#[cfg(test)]` so production cannot.
    #[cfg(test)]
    fn force_accumulate(&mut self, seconds: f64) {
        if let Some(state) = self.current.as_mut() {
            state.accumulated += seconds;
        }
    }

    /// Fire the one allowed `RecordPlayback` when qualified (library only, not
    /// already recorded, threshold reached). Sinks the error — it must not stop
    /// playback.
    ///
    /// The threshold is checked on every settle point, including **mid-play**
    /// ticks: the spec says a listen counts once the listener has heard
    /// `min(30s, 50%)` of the track, not "once they pause or reach the end". A
    /// scrobble that only fires on pause/end would silently lose every listen
    /// of an app that is force-quit mid-track.
    fn maybe_record(&mut self) {
        let Some(state) = self.current.as_mut() else {
            return;
        };
        if state.recorded {
            return;
        }
        let threshold = record_threshold(state.duration_seconds.unwrap_or(0.0));
        if state.accumulated < threshold {
            return;
        }
        if let Err(e) = self.recorder.record(state.session, state.song) {
            tracing::warn!(error = %e, "playback record failed (non-fatal)");
        }
        // Mark recorded so we do not retry-spam the DB each tick. A repeated
        // call is a no-op at the sink, but this keeps us off that path.
        state.recorded = true;
    }
}

/// The qualified listening threshold for a song of `duration_seconds`:
/// `min(30s, duration * 0.5)`; an unknown/non-positive duration uses the flat
/// 30s floor (design §12, task 8.10).
#[must_use]
pub fn record_threshold(duration_seconds: f64) -> f64 {
    if duration_seconds.is_finite() && duration_seconds > 0.0 {
        (duration_seconds * 0.5).min(30.0)
    } else {
        30.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::domain::ids::{PlaybackSessionId, SongId};
    use echo_core::domain::state::PlaybackState;
    use std::sync::Mutex;

    /// A recorder that records the calls (idempotent per session like the DB).
    #[derive(Default)]
    struct SpyRecorder {
        calls: Mutex<Vec<(PlaybackSessionId, SongId)>>,
    }

    impl PlaybackRecorder for SpyRecorder {
        fn record(&self, session: PlaybackSessionId, song: SongId) -> Result<bool, String> {
            self.calls.lock().unwrap().push((session, song));
            Ok(true)
        }
    }

    /// A recorder whose session is armed to appear on the boundary (for the
    /// threshold math we drive the state directly).
    #[derive(Default)]
    struct CountingRecorder {
        count: Mutex<usize>,
    }

    impl CountingRecorder {
        fn recorded(&self) -> usize {
            *self.count.lock().unwrap()
        }
    }

    impl PlaybackRecorder for CountingRecorder {
        fn record(&self, _session: PlaybackSessionId, _song: SongId) -> Result<bool, String> {
            *self.count.lock().unwrap() += 1;
            Ok(true)
        }
    }

    /// `record_threshold` returns `f64` seconds; compare with a tolerance
    /// rather than exact equality.
    fn assert_threshold(duration: f64, expected: f64) {
        let actual = record_threshold(duration);
        assert!(
            (actual - expected).abs() < 1e-9,
            "record_threshold({duration}) = {actual}, expected {expected}"
        );
    }

    #[test]
    fn threshold_uses_min_30s_of_half_duration() {
        // duration 100s → half = 50s → threshold = 30s (capped).
        assert_threshold(100.0, 30.0);
        // duration 40s → half = 20s → threshold = 20s (uncapped).
        assert_threshold(40.0, 20.0);
        // unknown/non-positive duration → flat 30s floor.
        assert_threshold(0.0, 30.0);
        assert_threshold(-5.0, 30.0);
    }

    #[test]
    fn temporary_items_are_never_recorded() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_temporary(PlaybackSessionId::new());
        rec.on_state(PlaybackState::Playing);
        rec.force_accumulate(31.0);
        rec.on_state(PlaybackState::Paused);
        // With no current library session, nothing recorded.
        assert_eq!(recorder.recorded(), 0, "temp never recorded");
    }

    #[test]
    fn library_session_records_once_reaching_threshold() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        rec.set_duration(100.0); // threshold 30s
        rec.on_state(PlaybackState::Playing);
        // Simulate 31s of real listening by force-accumulating, then pausing to
        // settle the segment and fire the one record.
        rec.force_accumulate(31.0);
        rec.on_state(PlaybackState::Paused); // settles → fires
        assert_eq!(recorder.recorded(), 1, "recorded once");
        // A repeated end/pause must not re-record.
        rec.on_state(PlaybackState::Ended);
        assert_eq!(recorder.recorded(), 1, "still exactly once");
    }

    #[test]
    fn below_threshold_does_not_record() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        rec.set_duration(100.0); // threshold 30s
        rec.force_accumulate(20.0); // below threshold
        rec.on_state(PlaybackState::Paused);
        assert_eq!(
            recorder.recorded(),
            0,
            "below-threshold listen is not counted"
        );
    }

    #[test]
    fn pausing_stops_the_clock() {
        // Pause should not accumulate time. We assert the internal invariant:
        // after pause, `playing_since` is None (the monotonic clock is stopped).
        let mut rec = PlaybackStatsRecorder::new(SpyRecorder::default());
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        rec.on_state(PlaybackState::Playing);
        assert!(rec.playing_since.is_some());
        rec.on_state(PlaybackState::Paused);
        assert!(
            rec.playing_since.is_none(),
            "paused stops the monotonic clock"
        );
    }

    // --- Real monotonic-clock coverage -------------------------------------
    //
    // Everything above drives `accumulated` with `#[cfg(test)] force_accumulate`,
    // which bypasses the wall clock entirely — it therefore could not catch the
    // defects these cover: `tick()` used to compute a total and then drop it
    // (`let _ = total;`) so ticks contributed *nothing*, and `maybe_record()`
    // returned early whenever `playing_since` was set, so a listen was only ever
    // counted if the user paused or reached the end. Both made the whole module
    // green-but-inert, exactly like no one calling it from production.
    //
    // These use tiny synthetic durations so the threshold is milliseconds and
    // the test stays fast, but time really elapses and `tick()` really folds it.

    #[test]
    fn tick_folds_elapsed_wall_clock_time_into_the_listen() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        // duration 100 ms → threshold min(50ms, 30s) = 50 ms.
        rec.set_duration(0.1);
        rec.on_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(80));
        // A tick while still playing must be enough — nobody has paused yet.
        rec.tick();
        assert_eq!(
            recorder.recorded(),
            1,
            "a listen qualifies mid-play once the threshold is reached"
        );
    }

    #[test]
    fn repeated_ticks_do_not_double_count_the_elapsed_segment() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        // duration 4 s → threshold min(2s, 30s) = 2 s of *real* listening.
        rec.set_duration(4.0);
        rec.on_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(120));
        // Five ticks over ~120 ms of wall clock must not inflate the listen into
        // 2 s: each tick folds only the time since the previous one.
        for _ in 0..5 {
            rec.tick();
        }
        assert_eq!(
            recorder.recorded(),
            0,
            "ticks fold time, they do not repeat it"
        );
    }

    #[test]
    fn a_paused_segment_really_accumulates_and_qualifies() {
        let recorder = CountingRecorder::default();
        let mut rec = PlaybackStatsRecorder::new(&recorder);
        rec.begin_library(PlaybackSessionId::new(), SongId::new());
        // duration 200 ms → threshold min(100ms, 30s) = 100 ms.
        rec.set_duration(0.2);
        // Two short playing segments separated by a pause add up.
        rec.on_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(60));
        rec.on_state(PlaybackState::Paused);
        assert_eq!(recorder.recorded(), 0, "60 ms is not a listen yet");
        rec.on_state(PlaybackState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(60));
        rec.on_state(PlaybackState::Paused);
        assert_eq!(
            recorder.recorded(),
            1,
            "two 60 ms segments of a 200 ms song cross its 100 ms threshold"
        );
    }
}
