//! Durable publication primitives with a fault hook at each native boundary.
use super::{invalid, Hook};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use crate::infrastructure::filesystem::{portable_migration as portable, RootRegistry};
use std::io::Write as _;

pub(super) fn publish(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
    exclusive: bool,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let path = portable::path(registry, root, relative)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing publication parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| Error::io("create durable directory", e, parent))?;
    let path = portable::path(registry, root, relative)?;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| Error::io("create durable temporary", e, &temporary))?;
        file.write_all(bytes)
            .map_err(|e| Error::io("write durable temporary", e, &temporary))?;
        file.sync_all()
            .map_err(|e| Error::io("sync durable temporary", e, &temporary))?;
        hook(&format!("{label}:file_sync"), 0)?;
        portable::path(registry, root, relative)?;
        if exclusive {
            // Atomic exclusive publication never exposes a truncated prepared file.
            std::fs::hard_link(&temporary, &path)
                .map_err(|e| Error::io("publish exclusive durable file", e, &path))?;
        } else {
            std::fs::rename(&temporary, &path)
                .map_err(|e| Error::io("replace durable file", e, &path))?;
        }
        hook(&format!("{label}:rename"), 0)?;
        sync_ancestors(registry, root, parent, label, hook)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

pub(super) fn sync_ancestors(
    registry: &RootRegistry,
    root: LibraryRootId,
    parent: &std::path::Path,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let base = registry.path_of(root)?;
    let mut directory = Some(parent);
    let mut index = 0;
    while let Some(path) = directory {
        super::sync_directory(path)?;
        hook(&format!("{label}:directory_sync"), index)?;
        if path == base {
            return Ok(());
        }
        directory = path.parent();
        index += 1;
    }
    Err(invalid("publication escaped registered root"))
}
