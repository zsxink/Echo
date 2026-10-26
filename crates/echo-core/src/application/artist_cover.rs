//! User-selected artist artwork saves library originals before updating its view.
use super::ports::{ControlPlanePort, CoverCache, UnitOfWork};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::ArtistCoverState;
use crate::domain::library::{DeviceId, PortableAssetReference};
use crate::error::Error;

/// # Errors
/// Invalid artwork, unavailable adapters and saved-pending failures propagate.
pub fn set_artist_cover(
    control: &dyn ControlPlanePort,
    unit: &dyn UnitOfWork,
    root: LibraryRootId,
    artist_key: &str,
    bytes: Option<&[u8]>,
    mime: Option<&str>,
    device: DeviceId,
) -> Result<(), Error> {
    ArtistCoverState::validate_artist_key(artist_key)?;
    let edits = unit
        .artist_cover_edits()
        .ok_or_else(|| Error::unavailable("artist artwork", "ordered adapter missing"))?;
    let asset = bytes
        .map(|bytes| super::artwork::save(control, root, bytes, mime.unwrap_or_default()))
        .transpose()?;
    edits.set_library_artist_cover(control, root, artist_key, asset, device)
}

/// Rebuild cache bytes from a saved original, without creating a new mutation.
/// Historical cache-only references remain readable while their bytes exist.
/// # Errors
/// Missing/corrupt originals and unavailable historical cache entries are explicit.
pub fn resolve_artist_cover(
    control: &dyn ControlPlanePort,
    cache: &dyn CoverCache,
    root: LibraryRootId,
    asset: Option<&PortableAssetReference>,
    legacy_key: Option<&str>,
) -> Result<Option<String>, Error> {
    if let Some(asset) = asset {
        return super::artwork::resolve(control, cache, root, asset).map(Some);
    }
    super::artwork::resolve_legacy(cache, legacy_key)
}
