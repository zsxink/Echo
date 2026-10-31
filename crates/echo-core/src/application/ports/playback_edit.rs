//! Serialized recording of qualified listens and their private event identities.
use super::ControlPlanePort;
use crate::domain::ids::{LibraryRootId, PlaybackSessionId, SongId};
use crate::domain::library::DeviceId;
use crate::error::Error;

pub trait PlaybackEditPort: Send + Sync {
    /// Save one stable event and its absolute device buckets before projection.
    ///
    /// `qualified_at` is the caller's already-persisted intent time. Replanning
    /// a retry must reuse it so the frozen payload stays identical.
    fn record_library_playback(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        session: PlaybackSessionId,
        song: SongId,
        device: DeviceId,
        qualified_at: u64,
    ) -> Result<bool, Error>;
}
