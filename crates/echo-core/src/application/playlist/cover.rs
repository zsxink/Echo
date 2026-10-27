//! Original playlist artwork is authoritative; cache entries are disposable.
use super::durable::DurableLibraryEdits;
use crate::application::ports::{ControlPlanePort, CoverCache};
use crate::domain::ids::{LibraryRootId, PlaylistId};
use crate::domain::library::{PortableRecord, RecordKind};
use crate::error::Error;

impl DurableLibraryEdits<'_> {
    /// Save the original before committing its reference; clearing retains history bytes.
    /// # Errors
    /// Invalid artwork, unknown playlist and saved-pending errors propagate.
    pub fn set_playlist_cover(
        &self,
        root: LibraryRootId,
        id: PlaylistId,
        bytes: Option<&[u8]>,
        mime: Option<&str>,
    ) -> Result<(), Error> {
        if !self.playlists.list(root)?.contains(&id) {
            return Err(Error::unavailable("playlist", "not in active library"));
        }
        let edits = self
            .unit
            .playlist_cover_edits()
            .ok_or_else(|| Error::unavailable("cover edit", "ordered adapter missing"))?;
        let asset = bytes
            .map(|bytes| {
                crate::application::artwork::save(
                    self.control,
                    root,
                    bytes,
                    mime.unwrap_or_default(),
                )
            })
            .transpose()?;
        edits.set_library_playlist_cover(self.control, root, id, asset, self.device)
    }
}

/// Resolve current durable artwork, rebuilding a missing cache without a new edit.
/// Historical cache-only evidence is readable but never claimed as an original.
/// # Errors
/// Missing/corrupt originals and unavailable historical cache entries are explicit.
pub fn resolve_playlist_cover(
    control: &dyn ControlPlanePort,
    cache: &dyn CoverCache,
    root: LibraryRootId,
    id: PlaylistId,
    legacy_key: Option<&str>,
) -> Result<Option<String>, Error> {
    let record = control.read_record(root, RecordKind::Playlist, &id.to_string())?;
    resolve_playlist_cover_record(control, cache, root, record.as_ref(), legacy_key)
}

/// Resolve an already loaded playlist record. This lets callers listing many
/// playlists read the directory's ordered records once instead of replaying the
/// directory separately for every cover.
///
/// # Errors
/// Missing or corrupt originals and unavailable cached artwork are reported.
pub fn resolve_playlist_cover_record(
    control: &dyn ControlPlanePort,
    cache: &dyn CoverCache,
    root: LibraryRootId,
    record: Option<&PortableRecord>,
    legacy_key: Option<&str>,
) -> Result<Option<String>, Error> {
    if let Some(PortableRecord::Playlist(record)) = record {
        if let Some(asset) = &record.cover_asset {
            return crate::application::artwork::resolve(control, cache, root, asset).map(Some);
        }
    }
    crate::application::artwork::resolve_legacy(cache, legacy_key)
}
