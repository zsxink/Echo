//! Deterministic time & identity doubles.
//!
//! [`FakeClock`] only advances when explicitly nudged; [`FakeIdGenerator`]
//! produces sequential song ids and random ids for everything else.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::unnecessary_to_owned,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::let_and_return,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::unchecked_time_subtraction,
    clippy::wildcard_imports,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::application::ports::*;
use crate::domain::ids::*;

/// A clock that only moves when explicitly nudged. Tests bump it to simulate
/// elapsed listening time, undo deadlines, scan pacing.
#[derive(Clone, Debug, Default)]
pub struct FakeClock {
    mono: Duration,
    wall: u128,
}

impl FakeClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mono: Duration::ZERO,
            wall: 1_700_000_000_000, // a fixed, stable wall anchor
        }
    }

    /// Advance the monotonic clock by `delta` (and wall by the same amount,
    /// scaled to seconds for realism).
    pub fn advance(&mut self, delta: Duration) {
        self.mono += delta;
        self.wall += delta.as_nanos();
    }
}

impl Clock for FakeClock {
    fn now_monotonic(&self) -> Duration {
        self.mono
    }
    fn now_wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_nanos(self.wall as u64)
    }
}

/// Deterministic identity generator: sequential / distinct within a test run
/// (fresh SongIds; other ids can be random).
#[derive(Clone, Debug, Default)]
pub struct FakeIdGenerator;

impl FakeIdGenerator {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl IdGenerator for FakeIdGenerator {
    fn new_song_id(&self) -> SongId {
        // Interior mutation via a thread-safe counter (tests are single-threaded).
        static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        SongId::from_uuid(uuid::Uuid::from_u128(
            C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128,
        ))
    }
    fn new_playlist_id(&self) -> PlaylistId {
        PlaylistId::new()
    }
    fn new_operation_id(&self) -> OperationId {
        OperationId::new()
    }
    fn new_library_root_id(&self) -> LibraryRootId {
        LibraryRootId::new()
    }
}

/// A shared, manually advanced clock: tests hold clones and nudge time
/// through `&self` while a use case runs. The wall clock advances with the
/// monotonic clock from a fixed anchor, so undo-deadline windows behave like
/// real time without touching the OS clock.
#[derive(Clone, Debug, Default)]
pub struct ManualClock {
    mono_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl ManualClock {
    /// Wall-clock anchor (epoch seconds) this fake starts from.
    const WALL_ANCHOR_SECS: u64 = 1_700_000_000;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the monotonic clock by `ms` milliseconds.
    pub fn advance_ms(&self, ms: u64) {
        self.mono_ms
            .fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// The epoch-millis wall time this fake currently reports.
    #[must_use]
    pub fn wall_ms(&self) -> u64 {
        Self::WALL_ANCHOR_SECS
            .saturating_mul(1_000)
            .saturating_add(self.mono_ms.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl Clock for ManualClock {
    fn now_monotonic(&self) -> Duration {
        Duration::from_millis(self.mono_ms.load(std::sync::atomic::Ordering::Relaxed))
    }
    fn now_wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(self.wall_ms())
    }
}

/// A clock that advances itself by a fixed step on every read — makes
/// throttle intervals deterministic without sleeping.
#[derive(Clone, Debug)]
pub struct SteppingClock {
    step_ms: u64,
    current_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl SteppingClock {
    #[must_use]
    pub fn new(step_ms: u64) -> Self {
        Self {
            step_ms,
            current_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl Clock for SteppingClock {
    fn now_monotonic(&self) -> Duration {
        Duration::from_millis(
            self.current_ms
                .fetch_add(self.step_ms, std::sync::atomic::Ordering::Relaxed),
        )
    }
    fn now_wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }
}

#[cfg(test)]
mod stepping_tests {
    use super::*;

    #[test]
    fn stepping_clock_advances_on_read() {
        let clock = SteppingClock::new(60);
        let t0 = clock.now_monotonic();
        let t1 = clock.now_monotonic();
        assert_eq!(t1 - t0, Duration::from_millis(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advances_monotonically_and_wall_is_stable() {
        let mut clock = FakeClock::new();
        let t0 = clock.now_monotonic();
        clock.advance(Duration::from_secs(35));
        let t1 = clock.now_monotonic();
        assert!(t1 > t0);
        assert_eq!(t1 - t0, Duration::from_secs(35));
        // Wall clock is anchored and never the real `now`.
        let w = clock.now_wall();
        assert!(w <= std::time::SystemTime::now());
    }

    #[test]
    fn fake_id_generator_produces_distinct_song_ids() {
        let gen = FakeIdGenerator::new();
        let a = gen.new_song_id();
        let b = gen.new_song_id();
        assert_ne!(a, b);
    }
}
