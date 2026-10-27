//! Durable publication primitives with a fault hook at each native boundary.
use super::{Hook, invalid};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use crate::infrastructure::filesystem::{RootRegistry, portable_migration as portable};
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub(super) fn publish(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
    exclusive: bool,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    publish_inner(
        registry, root, relative, bytes, exclusive, label, hook, None,
    )
}

/// Publish a file whose namespace durability will be established by the caller
/// before publishing any document that references it. This proof is local to a
/// single operation; callers must discard it on any failure.
#[allow(clippy::too_many_arguments)]
pub(super) fn publish_batched(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
    exclusive: bool,
    label: &str,
    hook: &mut Hook<'_>,
    directories: &mut DirectorySyncSet,
) -> Result<(), Error> {
    publish_inner(
        registry,
        root,
        relative,
        bytes,
        exclusive,
        label,
        hook,
        Some(directories),
    )
}

#[allow(clippy::too_many_arguments)]
fn publish_inner(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
    exclusive: bool,
    label: &str,
    hook: &mut Hook<'_>,
    directories: Option<&mut DirectorySyncSet>,
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
        directories.map_or_else(
            || sync_ancestors(registry, root, parent, label, hook),
            |directories| directories.include_ancestors(registry, root, parent),
        )
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Re-establish durability of an already visible publication. Equal bytes or a
/// completed rename do not prove its file and ancestor directories were synced.
pub(super) fn flush_existing(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let path = flush_file(registry, root, relative, label, hook)?;
    sync_ancestors(
        registry,
        root,
        path.parent()
            .ok_or_else(|| invalid("missing publication parent"))?,
        label,
        hook,
    )
}

/// File-only proof for a phase that will flush its complete directory set before
/// any namespace mutation. This must never replace a publication's own barriers.
pub(super) fn flush_file(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<PathBuf, Error> {
    let path = portable::path(registry, root, relative)?;
    // Windows FlushFileBuffers needs a writable handle. Unix can flush the
    // immutable publication through a read-only handle.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(&path)
        .map_err(|e| Error::io("open existing durable file", e, &path))?;
    if !file
        .metadata()
        .map_err(|e| Error::io("inspect durable file", e, &path))?
        .is_file()
    {
        return Err(invalid("durable publication is not a regular file"));
    }
    file.sync_all()
        .map_err(|e| Error::io("flush existing durable file", e, &path))?;
    hook(&format!("{label}:file_sync"), 0)?;
    portable::path(registry, root, relative)
}

/// Collect affected directories for a call-local durability proof. The proof
/// begins after `sync` returns and expires on the next namespace mutation.
/// Descendants are synced before their shared ancestors.
#[derive(Default)]
pub(super) struct DirectorySyncSet(BTreeSet<PathBuf>);

impl DirectorySyncSet {
    pub(super) fn include_ancestors(
        &mut self,
        registry: &RootRegistry,
        root: LibraryRootId,
        parent: &Path,
    ) -> Result<(), Error> {
        let base = registry.path_of(root)?;
        if !parent.starts_with(&base) {
            return Err(invalid("publication escaped registered root"));
        }
        for directory in parent.ancestors() {
            self.0.insert(directory.to_path_buf());
            if directory == base {
                return Ok(());
            }
        }
        Err(invalid("publication escaped registered root"))
    }

    pub(super) fn sync(
        self,
        registry: &RootRegistry,
        root: LibraryRootId,
        label: &str,
        hook: &mut Hook<'_>,
    ) -> Result<(), Error> {
        let mut directories: Vec<_> = self.0.into_iter().collect();
        directories.sort_by(|a, b| {
            b.components()
                .count()
                .cmp(&a.components().count())
                .then_with(|| a.cmp(b))
        });
        for (index, path) in directories.into_iter().enumerate() {
            // Hooks and filesystem changes must not turn a collected native
            // path into authority to follow a newly replaced symlink.
            let base = registry.path_of(root)?;
            let relative = path
                .strip_prefix(&base)
                .map_err(|_| invalid("publication escaped registered root"))?;
            let checked = if relative.as_os_str().is_empty() {
                // Resolving any child validates the root itself as well.
                portable::path(registry, root, "echo")?
                    .parent()
                    .ok_or_else(|| invalid("missing registered root"))?
                    .to_path_buf()
            } else {
                portable::path(
                    registry,
                    root,
                    &relative
                        .to_str()
                        .ok_or_else(|| invalid("non-UTF8 publication directory"))?
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                )?
            };
            if checked != path {
                return Err(invalid("publication root changed during directory proof"));
            }
            super::sync_directory(&checked)?;
            hook(&format!("{label}:directory_sync"), index)?;
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests;
