//! The root-id → absolute-path registry shared by the file-system adapters.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::domain::ids::LibraryRootId;
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
