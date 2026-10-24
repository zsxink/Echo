//! Serialized recording of qualified listens and their private event identities.
use super::ControlPlanePort;
use crate::domain::ids::{LibraryRootId, PlaybackSessionId, SongId};
use crate::domain::library::DeviceId;
use crate::error::Error;

pub trait PlaybackEditPort: Send + Sync {
    /// Save one stable event and its absolute device buckets before projection.
    fn record_library_playback(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        session: PlaybackSessionId,
        song: SongId,
        device: DeviceId,
    ) -> Result<bool, Error>;
}
