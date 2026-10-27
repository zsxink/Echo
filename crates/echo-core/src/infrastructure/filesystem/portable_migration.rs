//! Portable-directory (`echo/`) IO primitives: bounded reads, atomic
//! publishes, and typed UUID record enumeration.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use serde::Serialize;

use super::RootRegistry;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::library::{PortableRecord, RecordKind};
use crate::error::Error;

pub(super) const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
const MANIFEST: &str = "echo/manifest.json";

fn invalid(reason: &str) -> Error {
    Error::conflict(format!("portable migration: {reason}"))
}

pub(super) fn path(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
) -> Result<PathBuf, Error> {
    registry.resolve_relative(root, &RelativeMediaPath::new(relative)?)
}

pub(super) fn record_relative(record: &PortableRecord) -> Result<String, Error> {
    record.object_key()?;
    let uuid = record.object_uuid();
    let deleted_kind = match record {
        PortableRecord::Tombstone(value) => Some(value.deleted_kind),
        _ => None,
    };
    Ok(original_relative(record.kind(), uuid, deleted_kind))
}

pub(super) fn original_relative(
    kind: RecordKind,
    uuid: uuid::Uuid,
    tombstone_kind: Option<RecordKind>,
) -> String {
    let id = uuid.to_string();
    let typed = tombstone_kind.map_or_else(String::new, |kind| format!("{}/", kind.dir_name()));
    format!(
        "echo/records/{}/{typed}{}/{id}.json",
        kind.dir_name(),
        &id[..2]
    )
}

pub(super) fn read(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    limit: usize,
) -> Result<Option<String>, Error> {
    let target = path(registry, root, relative)?;
    let file = match std::fs::File::open(&target) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io("open portable file", error, &target)),
    };
    if !file
        .metadata()
        .map_err(|error| Error::io("inspect portable file", error, &target))?
        .is_file()
    {
        return Err(invalid("portable file is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::io("read portable file", error, &target))?;
    if bytes.len() > limit {
        return Err(invalid("portable file exceeds its size limit"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid("portable JSON is not UTF-8"))
}

pub(super) fn write(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
) -> Result<(), Error> {
    let target = path(registry, root, relative)?;
    let parent = target.parent().ok_or_else(|| invalid("missing parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io("create portable directory", error, parent))?;
    // Recheck ancestors after creating them, before any file is opened.
    let target = path(registry, root, relative)?;
    let temporary = target.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| Error::io("create portable temporary", error, &temporary))?;
        file.write_all(bytes)
            .map_err(|error| Error::io("write portable temporary", error, &temporary))?;
        file.sync_all()
            .map_err(|error| Error::io("sync portable temporary", error, &temporary))?;
        path(registry, root, relative)?;
        std::fs::rename(&temporary, &target)
            .map_err(|error| Error::io("publish portable file", error, &target))?;
        sync_directory(parent)
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg_attr(
    windows,
    allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)
)]
fn sync_directory(directory: &std::path::Path) -> Result<(), Error> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| Error::io("sync portable directory", error, directory))?;
    }
    #[cfg(windows)]
    let _ = directory;
    Ok(())
}

pub(super) fn remove(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
) -> Result<(), Error> {
    let target = path(registry, root, relative)?;
    match std::fs::remove_file(&target) {
        Ok(()) => sync_directory(target.parent().ok_or_else(|| invalid("missing parent"))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::io("remove portable file", error, target)),
    }
}

pub(super) fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, Error> {
    struct BoundedJson {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for BoundedJson {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "portable JSON exceeds size limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = BoundedJson {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value).map_err(|error| {
        Error::unavailable_with_source("portable migration", "cannot encode bounded JSON", error)
    })?;
    Ok(output.bytes)
}

/// Enumerate only bounded, typed UUID record paths. Never follow a symlink,
/// ignore an unreadable entry, or turn a malformed record into a lost object.
pub(super) fn record_files(
    registry: &RootRegistry,
    root: LibraryRootId,
    kind: RecordKind,
) -> Result<Vec<String>, Error> {
    let mut result = Vec::new();
    let directory = format!("echo/records/{}", kind.dir_name());
    collect(registry, root, &directory, 0, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    depth: usize,
    result: &mut Vec<String>,
) -> Result<(), Error> {
    let directory = path(registry, root, relative)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io("list portable records", error, directory)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::io("read portable record entry", error, &directory))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("non UTF-8 record path"))?;
        if name.contains(".tmp-") {
            continue;
        }
        let child = format!("{relative}/{name}");
        let target = path(registry, root, &child)?;
        let metadata = std::fs::symlink_metadata(&target)
            .map_err(|error| Error::io("inspect portable record", error, &target))?;
        if metadata.is_dir() {
            if depth >= 2 {
                return Err(invalid("record directory is too deep"));
            }
            collect(registry, root, &child, depth + 1, result)?;
        } else if std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            if result.len() >= 100_000 {
                return Err(invalid("too many portable records"));
            }
            result.push(child);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
