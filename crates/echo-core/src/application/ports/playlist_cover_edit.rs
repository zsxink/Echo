//! Ordered edits of playlist original-asset references.
use super::ControlPlanePort;
use crate::domain::ids::{LibraryRootId, PlaylistId};
use crate::domain::library::{DeviceId, PortableAssetReference};
use crate::error::Error;

pub trait PlaylistCoverEditPort: Send + Sync {
    /// Freeze the complete playlist and original resource before SQL projection.
    fn set_library_playlist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error>;
}
