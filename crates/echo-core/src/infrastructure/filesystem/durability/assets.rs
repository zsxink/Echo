//! Verified resource declarations and immutable user originals.
use super::{invalid, io, portable, Hook};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use crate::infrastructure::filesystem::RootRegistry;

pub(super) fn validate_resource(
    registry: &RootRegistry,
    root: LibraryRootId,
    resource: &crate::domain::library::durability::DurableResource,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    use std::io::Read as _;
    let path = portable::path(registry, root, &resource.relative_path)?;
    let mut file =
        std::fs::File::open(&path).map_err(|e| Error::io("open durable resource", e, &path))?;
    let metadata = file
        .metadata()
        .map_err(|e| Error::io("inspect durable resource", e, &path))?;
    if !metadata.is_file() || metadata.len() != resource.byte_length {
        return Err(invalid("durable resource length mismatch"));
    }
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| Error::io("verify durable resource", e, &path))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if hash.finalize().to_hex().as_str() != resource.content_hash {
        return Err(invalid("durable resource hash mismatch"));
    }
    file.sync_all()
        .map_err(|e| Error::io("flush durable resource", e, &path))?;
    hook("resource:file_sync", 0)?;
    io::sync_ancestors(
        registry,
        root,
        path.parent()
            .ok_or_else(|| invalid("resource parent missing"))?,
        "resource",
        hook,
    )
}

pub(in crate::infrastructure::filesystem) fn write_asset(
    registry: &RootRegistry,
    root: LibraryRootId,
    bytes: &[u8],
) -> Result<String, Error> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(invalid("user asset exceeds 32 MiB"));
    }
    let hash = blake3::hash(bytes).to_hex().to_string();
    let relative = format!("echo/assets/{hash}");
    if let Some(existing) = read_asset(registry, root, &hash)? {
        if existing != bytes {
            return Err(invalid("immutable asset collision"));
        }
    } else {
        io::publish(
            registry,
            root,
            &relative,
            bytes,
            true,
            "asset",
            &mut |_, _| Ok(()),
        )?;
    }
    Ok(hash)
}
pub(in crate::infrastructure::filesystem) fn read_asset(
    registry: &RootRegistry,
    root: LibraryRootId,
    hash: &str,
) -> Result<Option<Vec<u8>>, Error> {
    use std::io::Read as _;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("invalid asset hash"));
    }
    let path = portable::path(registry, root, &format!("echo/assets/{hash}"))?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io("read library asset", e, path)),
    };
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io("read immutable asset", e, &path))?;
    if bytes.len() > 32 * 1024 * 1024 || blake3::hash(&bytes).to_hex().as_str() != hash {
        return Err(invalid("immutable asset is corrupt"));
    }
    Ok(Some(bytes))
}
