//! Shared validation and reconstruction of content-addressed artwork originals.
use super::ports::{ControlPlanePort, CoverCache};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::PortableAssetReference;
use crate::error::{Error, Subject};

pub fn validate(bytes: &[u8], mime: &str) -> Result<(), Error> {
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
