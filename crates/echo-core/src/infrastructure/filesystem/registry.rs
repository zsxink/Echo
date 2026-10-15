//! The root-id → absolute-path registry shared by the file-system adapters.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::error::Error;

/// Maps a [`LibraryRootId`] to its absolute directory on this machine.
///
/// The desktop composition root registers a root when it is prepared or
/// activated and removes it when the root is unbound; adapters never read the
/// database for paths and use cases never see one. The registry deliberately
/// stores the path as given — canonicalization is a per-operation check so a
/// root whose directory is swapped behind Echo fails the containment check
/// instead of silently following the swap.
#[derive(Clone, Debug, Default)]
pub struct RootRegistry {
    roots: Arc<Mutex<HashMap<LibraryRootId, PathBuf>>>,
}

impl RootRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or re-point) a root. Called by the runtime / tests.
    ///
    /// # Panics
    ///
    /// Only if the registry mutex is poisoned (a prior panic while holding
    /// it), which indicates a bug in this adapter.
    pub fn register(&self, root: LibraryRootId, path: impl Into<PathBuf>) {
        self.roots.lock().unwrap().insert(root, path.into());
    }

    /// Remove a root (the runtime dropped its binding).
    ///
    /// # Panics
    ///
    /// Only if the registry mutex is poisoned (see [`Self::register`]).
    pub fn unregister(&self, root: LibraryRootId) {
        self.roots.lock().unwrap().remove(&root);
    }

    /// The absolute path of a root.
    ///
    /// # Panics
    ///
    /// Only if the registry mutex is poisoned (see [`Self::register`]).
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the root is not currently registered — the
    /// same shape as a root that vanished between prepare and use.
    pub fn path_of(&self, root: LibraryRootId) -> Result<PathBuf, Error> {
        self.roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .ok_or_else(|| Error::unavailable("library root", "root is not bound"))
    }

    /// Resolve a relative path without traversing symbolic links or Windows
    /// reparse points, including links in intermediate directories. Missing
    /// components are allowed for exclusive-create publication destinations.
    ///
    /// # Errors
    /// Fails closed on an unbound root, symlink/reparse point, unreadable
    /// ancestor, or an existing ancestor outside the canonical root.
    pub fn resolve_relative(
        &self,
        root: LibraryRootId,
        relative: &RelativeMediaPath,
    ) -> Result<PathBuf, Error> {
        let base = self.path_of(root)?;
        let root_meta = std::fs::symlink_metadata(&base)
            .map_err(|source| Error::io("inspect library root", source, &base))?;
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Err(Error::permission(
                "resolve library root",
                crate::error::PermKind::NotOwner,
            ));
        }
        let canonical_root = base
            .canonicalize()
            .map_err(|source| Error::io("canonicalize library root", source, &base))?;
        let mut path = base;
        for component in relative.normalized().split('/') {
            path.push(component);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(Error::permission(
                            "resolve library path",
                            crate::error::PermKind::NotOwner,
                        ));
                    }
                    let canonical = path
                        .canonicalize()
                        .map_err(|source| Error::io("canonicalize library path", source, &path))?;
                    if !canonical.starts_with(&canonical_root) {
                        return Err(Error::permission(
                            "library path containment",
                            crate::error::PermKind::NotOwner,
                        ));
                    }
                }
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    reject_portable_collision(&path, component)?;
                }
                Err(source) => return Err(Error::io("inspect library path", source, &path)),
            }
        }
        Ok(path)
    }

    /// Every registered root (diagnostics / shutdown only).
    ///
    /// # Panics
    ///
    /// Only if the registry mutex is poisoned (see [`Self::register`]).
    #[must_use]
    pub fn registered(&self) -> Vec<(LibraryRootId, PathBuf)> {
        self.roots
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }
}

/// A case-sensitive filesystem must not create two names that would address
/// one file on Windows or macOS. Existing exact paths remain readable; a new
/// spelling that aliases a foreign entry fails before exclusive creation.
fn reject_portable_collision(path: &std::path::Path, component: &str) -> Result<(), Error> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(Error::io("inspect destination names", source, parent)),
    };
    let identity = crate::domain::text::path_identity_key(component);
    for entry in entries {
        let entry =
            entry.map_err(|source| Error::io("inspect destination name", source, parent))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| crate::domain::text::path_identity_key(name) == identity)
        {
            return Err(Error::conflict(
                "library path collides by case or Unicode normalization",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trips_and_reports_unknown_roots() {
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        let dir = tempfile::tempdir().unwrap();
        registry.register(root, dir.path());
        assert_eq!(registry.path_of(root).unwrap(), dir.path());
        registry.unregister(root);
        let error = registry.path_of(root).unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
}
