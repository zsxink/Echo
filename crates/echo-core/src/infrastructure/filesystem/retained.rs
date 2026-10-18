//! Verified immutable copies kept outside disposable operation staging.

use super::RootRegistry;
use crate::application::ports::RetainedResource;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::sync::ResourceDigest;
use crate::error::Error;
use std::io::{Read, Write};

pub fn copy(
    registry: &RootRegistry,
    root: LibraryRootId,
    source: &RelativeMediaPath,
    expected: &str,
) -> Result<RetainedResource, Error> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::conflict("invalid retained resource digest"));
    }
    let source = registry.resolve_relative(root, source)?;
    let metadata = source
        .symlink_metadata()
        .map_err(|error| Error::io("inspect retained source", error, source.clone()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::conflict("retained source is not a regular file"));
    }
    let relative = RelativeMediaPath::new(&format!("echo/media/retained/{expected}"))?;
    let target = registry.resolve_relative(root, &relative)?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::conflict("invalid retention target"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io("create retention directory", error, parent))?;
    // Re-resolve after directory creation to reject symlinked ancestors.
    let target = registry.resolve_relative(root, &relative)?;
    if target.exists() {
        let (digest, length) = verify(&target)?;
        if digest.to_hex() != expected || length != metadata.len() {
            return Err(Error::conflict(
                "retained resource differs from expected content",
            ));
        }
        return Ok(RetainedResource {
            digest,
            length,
            path: relative,
        });
    }
    let part = target.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut input = std::fs::File::open(&source)
            .map_err(|error| Error::io("open retained source", error, source.clone()))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|error| Error::io("create retained copy", error, part.clone()))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 128 * 1024];
        let mut length = 0u64;
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|error| Error::io("read retained source", error, source.clone()))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| Error::io("write retained copy", error, part.clone()))?;
            hasher.update(&buffer[..read]);
            length += read as u64;
        }
        let digest = ResourceDigest::from_bytes(*hasher.finalize().as_bytes());
        if digest.to_hex() != expected || length != metadata.len() {
            return Err(Error::conflict(
                "retention source changed before durable copy",
            ));
        }
        output
            .sync_all()
            .map_err(|error| Error::io("sync retained copy", error, part.clone()))?;
        // A hard link publishes exclusively. A racing writer can never be overwritten.
        match std::fs::hard_link(&part, &target) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if verify(&target)? != (digest, length) {
                    return Err(Error::conflict(
                        "racing retained copy has different content",
                    ));
                }
            }
            Err(error) => return Err(Error::io("publish retained copy", error, target.clone())),
        }
        sync_directory(parent)?;
        Ok(RetainedResource {
            digest,
            length,
            path: relative,
        })
    })();
    let _ = std::fs::remove_file(&part);
    result
}

fn verify(path: &std::path::Path) -> Result<(ResourceDigest, u64), Error> {
    let metadata = path
        .symlink_metadata()
        .map_err(|error| Error::io("inspect retained copy", error, path))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::conflict("retained copy is not a regular file"));
    }
    let mut input =
        std::fs::File::open(path).map_err(|error| Error::io("open retained copy", error, path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 128 * 1024];
    let mut length = 0;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| Error::io("verify retained copy", error, path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        length += read as u64;
    }
    Ok((
        ResourceDigest::from_bytes(*hasher.finalize().as_bytes()),
        length,
    ))
}

#[cfg(unix)]
fn sync_directory(path: &std::path::Path) -> Result<(), Error> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| Error::io("sync retention directory", error, path))
}
#[cfg(not(unix))]
fn sync_directory(_path: &std::path::Path) -> Result<(), Error> {
    Ok(())
}
