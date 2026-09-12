//! Production `Clock` and `IdGenerator` adapters (composition root).
//!
//! The desktop composition root needs a wall/monotonic clock and a UUID
//! generator to assemble a real `ScanDeps`. Until this module landed, only
//! test fakes existed (`testing/clock.rs`). These two adapters are pure,
//! platform-neutral, and unit-testable headlessly.

use std::time::{Duration, Instant, SystemTime};

use crate::application::ports::{Clock, IdGenerator};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};

/// A wall-clock implementation of [`Clock`]: monotonic from process start,
/// wall-clock absolute from the system clock.
#[derive(Clone, Copy, Debug)]
pub struct WallClock {
    /// Monotonic origin captured once so `now_monotonic` is elapsed-from-boot.
    origin: Instant,
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

impl WallClock {
    /// A clock whose monotonic base is "now".
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for WallClock {
    fn now_monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn now_wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A UUID-v4 [`IdGenerator`] for production identity.
///
/// Every id type wraps a fresh random UUID (see `domain/ids`), so the four
/// generator methods are trivially correct and collision-safe enough for on-box
/// catalogs. Determinism stays the job of the test fakes in `testing/clock.rs`.
#[derive(Clone, Copy, Debug, Default)]
pub struct UuidV4Generator;

impl IdGenerator for UuidV4Generator {
    fn new_song_id(&self) -> SongId {
        SongId::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_monotonic_never_goes_backward_and_wall_is_recent() {
        let clock = WallClock::new();
        let a = clock.now_monotonic();
        let b = clock.now_monotonic();
        // Monotonic must never regress; in practice the second read is equal or
        // later.
        assert!(b >= a);
        // Wall clock is close to the real system time (within a minute).
        let wall = clock.now_wall();
        let delta = SystemTime::now()
            .duration_since(wall)
            .unwrap_or_else(|e| e.duration());
        assert!(delta < Duration::from_secs(60));
    }

    #[test]
    fn uuid_generator_produces_distinct_ids_per_call_and_kind() {
        let gen = UuidV4Generator;
        assert_ne!(gen.new_song_id(), gen.new_song_id());
        assert_ne!(gen.new_playlist_id(), gen.new_playlist_id());
        assert_ne!(gen.new_operation_id(), gen.new_operation_id());
        assert_ne!(gen.new_library_root_id(), gen.new_library_root_id());
        // Cross-kind ids are distinct string representations.
        assert_ne!(
            gen.new_song_id().to_string(),
            gen.new_playlist_id().to_string()
        );
    }
}
