//! Shared validation and reconstruction of content-addressed artwork originals.
use std::io::Cursor;

use image::{ImageFormat, ImageReader};

use super::ports::{ControlPlanePort, CoverCache};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::PortableAssetReference;
use crate::error::{Error, Subject};

/// Cover MIME types accepted for a user-picked original. The durable write
/// guard below and the desktop shell's in-memory preview share this one list,
/// so a future format addition cannot make a cover savable but un-previewable.
pub const SUPPORTED_COVER_MIMES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

pub fn validate(bytes: &[u8], mime: &str) -> Result<(), Error> {
    if bytes.is_empty() || bytes.len() > 5 * 1024 * 1024 {
        return Err(Error::validation(
            Subject::Other,
            "cover",
            "image must be between 1 byte and 5 MiB",
        ));
    }
    if !SUPPORTED_COVER_MIMES.contains(&mime) {
        return Err(Error::validation(
            Subject::Other,
            "cover",
            "unsupported image type",
        ));
    }
    validate_content(bytes, mime)
}

/// Reject bytes that only *claim* to be a cover image before any durable asset
/// or cache entry is written. `save` and `resolve` share this guard so a bad
/// original can never reach `CoverCache::put`.
///
/// The probe is deliberately header-only: `into_dimensions()` builds a decoder
/// and reads container dimensions without allocating an RGBA pixel buffer.
/// A full decode would multiply a 5 MiB input into hundreds of MiB of pixels
/// (the same cost class as issue #133) and add a decompression-bomb surface.
fn validate_content(bytes: &[u8], mime: &str) -> Result<(), Error> {
    let invalid = |reason: &str| Error::validation(Subject::Other, "cover", reason);
    let unrecognized = || invalid("cover content is not a recognized image");
    let mismatched = || invalid("cover content does not match the declared MIME type");
    let bad_header = || invalid("cover image header is invalid or truncated");

    // `with_guessed_format` peeks only the leading signature bytes; an unknown
    // container leaves `format()` as `None` rather than returning an io error.
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| unrecognized())?;
    let detected = reader.format().ok_or_else(unrecognized)?;
    let declared = ImageFormat::from_mime_type(mime).ok_or_else(mismatched)?;
    if detected != declared || !detected.reading_enabled() {
        return Err(mismatched());
    }

    // `ImageReader` defaults to a 512 MiB allocation ceiling that is applied
    // while constructing the decoder. The save path never decodes pixels, so
    // that ceiling must not reject a legitimate large image here.
    reader.no_limits();
    let (width, height) = reader.into_dimensions().map_err(|_| bad_header())?;
    if width == 0 || height == 0 {
        return Err(bad_header());
    }
    Ok(())
}

pub fn save(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    bytes: &[u8],
    mime: &str,
) -> Result<PortableAssetReference, Error> {
    validate(bytes, mime)?;
    let durable = control
        .durability()
        .ok_or_else(|| Error::unavailable("cover edit", "directory adapter missing"))?;
    Ok(PortableAssetReference {
        content_hash: durable.write_asset(root, bytes)?,
        byte_len: u64::try_from(bytes.len())
            .map_err(|_| Error::conflict("cover length overflow"))?,
        mime: mime.to_owned(),
    })
}

pub fn resolve(
    control: &dyn ControlPlanePort,
    cache: &dyn CoverCache,
    root: LibraryRootId,
    asset: &PortableAssetReference,
) -> Result<String, Error> {
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
    cache.put(&bytes, &asset.mime)
}

pub fn resolve_legacy(
    cache: &dyn CoverCache,
    legacy_key: Option<&str>,
) -> Result<Option<String>, Error> {
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

#[cfg(test)]
mod tests;
