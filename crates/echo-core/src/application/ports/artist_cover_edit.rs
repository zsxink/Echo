//! Serialized local artwork edits save an original reference before projection.
use super::ControlPlanePort;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::{DeviceId, PortableAssetReference};
use crate::error::Error;

pub trait ArtistCoverEditPort: Send + Sync {
    fn set_library_artist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        artist_key: &str,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error>;
}
