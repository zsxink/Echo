//! Original playlist artwork is authoritative; cache entries are disposable.
use super::durable::DurableLibraryEdits;
use crate::application::ports::{ControlPlanePort, CoverCache};
use crate::domain::ids::{LibraryRootId, PlaylistId};
use crate::domain::library::{PortableAssetReference, PortableRecord, RecordKind};
use crate::error::{Error, Subject};

fn validate(bytes: &[u8], mime: &str) -> Result<(), Error> {
    if bytes.is_empty() || bytes.len() > 5 * 1024 * 1024 {
        return Err(Error::validation(
            Subject::Other,
            "cover",
            "image must be between 1 byte and 5 MiB",
        ));
    }
    if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
        return Err(Error::validation(
            Subject::Other,
            "cover",
            "unsupported image type",
        ));
    }
    Ok(())
}

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
                let mime = mime.unwrap_or_default();
                validate(bytes, mime)?;
                let durable = self
                    .control
                    .durability()
                    .ok_or_else(|| Error::unavailable("cover edit", "directory adapter missing"))?;
                Ok::<_, Error>(PortableAssetReference {
                    content_hash: durable.write_asset(root, bytes)?,
                    byte_len: u64::try_from(bytes.len())
                        .map_err(|_| Error::conflict("cover length overflow"))?,
                    mime: mime.to_owned(),
                })
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
    if let Some(PortableRecord::Playlist(record)) = record {
        if let Some(asset) = record.cover_asset {
            asset.validate()?;
            let durable = control
                .durability()
                .ok_or_else(|| Error::unavailable("cover original", "directory adapter missing"))?;
            let bytes = durable
                .read_asset(root, &asset.content_hash)?
                .ok_or_else(|| Error::unavailable("cover original", "original is missing"))?;
            validate(&bytes, &asset.mime)?;
            if u64::try_from(bytes.len()).ok() != Some(asset.byte_len)
                || blake3::hash(&bytes).to_hex().as_str() != asset.content_hash
            {
                return Err(Error::conflict(
                    "cover original differs from saved reference",
                ));
            }
            return cache.put(&bytes, &asset.mime).map(Some);
        }
    }
    if let Some(key) = legacy_key {
        if cache.get(key)?.is_none() {
            return Err(Error::unavailable(
                "cover original",
                "historical cover has no recoverable original",
            ));
        }
        return Ok(Some(key.to_owned()));
    }
    Ok(None)
}
