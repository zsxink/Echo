//! Library/Device identity ids and the hybrid-logical clock.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Subject};

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct LibraryId(uuid::Uuid);

impl LibraryId {
    /// Create from an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    /// The inner UUID for boundary marshalling.
    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for LibraryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for LibraryId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|_| Error::validation(Subject::Id, "LibraryId", "invalid UUID format"))?;
        Ok(Self(uuid))
    }
}

/// The identity of the device that wrote or last modified a portable object.
///
/// Persisted alongside the object's HLC and revision to enable deterministic
/// last-writer-wins conflict resolution in the sync engine (design §3-5).
///
/// Each device maintains exactly one `DeviceId`, generated randomly (UUID v4)
/// at first startup and stored in the application data directory.  It is
/// **never** regenerated and **never** carried in the `echo/` control surface
/// (it is local-only metadata).
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct DeviceId(uuid::Uuid);

impl DeviceId {
    /// Create a new random device identity.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Wrap an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    /// The inner UUID.
    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for DeviceId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|_| Error::validation(Subject::Id, "DeviceId", "invalid UUID format"))?;
        Ok(Self(uuid))
    }
}

// ── Hybrid Logical Clock ────────────────────────────────────────────────────

/// A Hybrid Logical Clock (HLC) timestamp for cross-device event ordering.
///
/// Combines wall-clock seconds since epoch with a monotonic counter to ensure
/// that no two events share the same timestamp-counter pair, and that the
/// ordering is consistent with both wall-clock time and logical precedence.
///
/// HLCs are **persistent and durable**: they are written to object records on
/// each mutation, never regenerated at runtime, and never go backward.  For
/// events on the *same* device the local `revision` is authoritative; the HLC
/// is the cross-device ordering signal.
///
/// Lexicographic comparison on `(wall_secs, counter)` gives a total order
/// suitable for the LWW conflict resolution in the future sync engine (design
/// §3-5: "从本 change 起持久化 HLC 与设备 ID，未来统一 LWW 规则").
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct HybridLogicalClock {
    /// Seconds since Unix epoch (never regresses within a device).
    pub wall_secs: u64,
    /// Monotonic counter that increments when the wall clock hasn't advanced.
    pub counter: u32,
}

impl HybridLogicalClock {
    /// Construct from explicit components.
    #[must_use]
    pub const fn new(wall_secs: u64, counter: u32) -> Self {
        Self { wall_secs, counter }
    }
}

/// The next HLC after `prev` observed at wall-clock second `now_wall_secs`.
///
/// The rule (HLC advance on the same device) keeps the clock monotone and never
/// regressing, so two consecutive local writes can always be ordered:
///
/// - wall time advanced → `(now_wall_secs, 0)`;
/// - otherwise the counter increments within the same wall second;
/// - and a saturated counter still advances the wall second (so the tuple
///   grows even if wall time did not move).
///
/// The loop-back case (`now_wall_secs < prev.wall_secs`, e.g. the system clock
/// stepped backwards) keeps `prev`'s wall second rather than stepping back.
#[must_use]
pub const fn next_hlc(prev: HybridLogicalClock, now_wall_secs: u64) -> HybridLogicalClock {
    if now_wall_secs > prev.wall_secs {
        HybridLogicalClock::new(now_wall_secs, 0)
    } else if prev.counter < u32::MAX {
        HybridLogicalClock::new(prev.wall_secs, prev.counter + 1)
    } else {
        HybridLogicalClock::new(prev.wall_secs.saturating_add(1), 0)
    }
}

impl fmt::Display for HybridLogicalClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.wall_secs, self.counter)
    }
}

impl std::str::FromStr for HybridLogicalClock {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (wall, counter) = s.split_once(':').ok_or_else(|| {
            Error::validation(Subject::Other, "HLC", "expected 'wall_secs:counter'")
        })?;
        let wall_secs = wall
            .parse::<u64>()
            .map_err(|_| Error::validation(Subject::Other, "HLC", "wall_secs must be a u64"))?;
        let counter = counter
            .parse::<u32>()
            .map_err(|_| Error::validation(Subject::Other, "HLC", "counter must be a u32"))?;
        Ok(Self { wall_secs, counter })
    }
}
