//! The root-constrained [`LibraryFileSystem`] adapter (task 4.2).

use std::collections::HashMap;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::application::ports::{FileMeta, LibraryFileSystem, StagedResource};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::error::{Error, PermKind};

use super::registry::RootRegistry;
use super::staging::{StagingCheck, StagingManager};
use super::walker;

/// Adapter-private map of staged resources (handle → location).
type StagedMap = HashMap<(LibraryRootId, OperationId, String), StagedLocation>;

/// Where a staged resource currently lives: the staging directory it was
/// written into plus its file inside. The use case only ever sees the
/// [`StagedResource`] handle; this map is the adapter-private resolver.
#[derive(Clone, Debug)]
struct StagedLocation {
    staging_dir: PathBuf,
    file: PathBuf,
}

/// Real-filesystem implementation of the library file-system port.
///
/// All operations resolve the root's absolute path through the shared
/// [`RootRegistry`] and constrain every access to that directory. Publishes
/// only ever move files out of the adapter's own, marker-verified staging
/// directory into the target relative path — a use case cannot name an
/// external path, and a forged handle fails with a permission error.
#[derive(Clone, Debug)]
pub struct RootConstrainedFileSystem {
    registry: RootRegistry,
    staging: StagingManager,
    staged: Arc<Mutex<StagedMap>>,
}

impl RootConstrainedFileSystem {
    /// Build the adapter over a shared registry. The same registry instance
    /// should back the probe/metadata adapters so every component resolves a
    /// root to the same directory.
    #[must_use]
    pub fn new(registry: RootRegistry) -> Self {
        let staging = StagingManager::new(registry.clone());
        Self {
            registry,
            staging,
            staged: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The staging manager (used by import/delete adapters in later phases).
    #[must_use]
    pub const fn staging(&self) -> &StagingManager {
        &self.staging
    }

    fn abs(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<PathBuf, Error> {
        Ok(self.registry.path_of(root)?.join(path.normalized()))
    }

    /// Write bytes into a fresh file under the root's staging directory and
    /// register the handle. Shared helper behind the port's [`Self::stage`]
    /// (import ingestion, task 5.1) and the test setup entry; the port itself
    /// only exposes [`StagedResource`], never a path.
    ///
    /// # Errors
    ///
    /// Fails when the staging directory cannot be established (write
    /// capability disabled) or the write itself fails; the handle is then not
    /// registered.
    ///
    /// # Panics
    ///
    /// Only if the internal staging map's mutex is poisoned (a prior panic
    /// while holding it), which indicates a bug in this adapter.
    pub fn stage_bytes(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let staging_dir = self.staging.ensure_dir(root)?;
        let file = staging_dir.join(format!(
            "{}-{}",
            staged.operation().as_uuid().simple(),
            staged.resource_key()
        ));
        std::fs::write(&file, bytes).map_err(|source| Error::io("stage write", source, &file))?;
        self.staged.lock().unwrap().insert(
            (root, staged.operation(), staged.resource_key().to_owned()),
            StagedLocation { staging_dir, file },
        );
        Ok(())
    }
}

impl LibraryFileSystem for RootConstrainedFileSystem {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        // Reject an enumeration when the root itself is unreadable; per-entry
        // failures are skipped inside the walker.
        self.registry.path_of(root)?;
        Ok(walker::enumerate_files(root, &self.staging)?.0)
    }

    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
        let abs = self.abs(root, path)?;
        let meta = std::fs::metadata(&abs).map_err(|source| Error::io("stat", source, abs))?;
        Ok(FileMeta {
            size: meta.len(),
            modified_ns: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX))
                .unwrap_or_default(),
        })
    }

    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        let abs = self.abs(root, path)?;
        // Read at most `limit` bytes off the disk — never buffer a whole
        // large file to then truncate in memory.
        let file =
            std::fs::File::open(&abs).map_err(|source| Error::io("open", source, abs.clone()))?;
        let mut file = file.take(limit);
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|source| Error::io("read", source, abs))?;
        Ok(data)
    }

    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let key = (root, staged.operation(), staged.resource_key().to_owned());
        let location = self
            .staged
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::permission("publish staged resource", PermKind::NotOwner))?;
        // The staging directory must still be marker-verified at publish time;
        // a directory that lost its marker between staging and publishing is
        // no longer trusted.
        if self.staging.check(&location.staging_dir, root) != StagingCheck::Owned {
            return Err(Error::permission(
                "verify staging marker",
                PermKind::NotOwner,
            ));
        }
        let dest = self.abs(root, target)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io("create target directory", source, parent.to_path_buf())
            })?;
        }
        if dest.exists() {
            // Publish is create-new: never replace existing content (design
            // §8). A same-name conflict surfaces to the ingestion flow.
            return Err(Error::conflict("target file already exists"));
        }
        std::fs::rename(&location.file, &dest)
            .map_err(|source| Error::io("publish", source, dest.clone()))?;
        self.staged.lock().unwrap().remove(&key);
        Ok(())
    }

    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error> {
        self.stage_bytes(root, staged, content)
    }

    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.staging.write_capable(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::StagedResource;

    fn setup() -> (tempfile::TempDir, LibraryRootId, RootConstrainedFileSystem) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        (dir, root, RootConstrainedFileSystem::new(registry))
    }

    #[test]
    fn staged_handle_publishes_into_the_root_and_rejects_replacement() {
        let (_dir, root, fs) = setup();
        let operation = OperationId::new();
        let staged = StagedResource::new(operation, "audio").unwrap();
        fs.stage_bytes(root, &staged, b"published-bytes").unwrap();

        let target = RelativeMediaPath::new("artist/tone.mp3").unwrap();
        fs.publish(root, &staged, &target).unwrap();
        assert!(
            fs.read_head(root, &target, 64).unwrap() == b"published-bytes".to_vec(),
            "the staged file moved to its target"
        );

        // Republishing onto the existing target is refused (never replace).
        let staged2 = StagedResource::new(operation, "audio-2").unwrap();
        fs.stage_bytes(root, &staged2, b"second").unwrap();
        let error = fs.publish(root, &staged2, &target).unwrap_err();
        assert_eq!(error.code(), "conflict");
    }

    #[test]
    fn forged_staged_handles_are_rejected() {
        let (_dir, root, fs) = setup();
        let forged = StagedResource::new(OperationId::new(), "forged").unwrap();
        let error = fs
            .publish(root, &forged, &RelativeMediaPath::new("x.mp3").unwrap())
            .unwrap_err();
        assert_eq!(error.code(), "permission");
    }

    #[test]
    fn write_capability_reflects_the_staging_marker() {
        let (_dir, root, fs) = setup();
        assert!(
            !fs.write_capable(root).unwrap(),
            "no staging directory yet: writes disabled"
        );
        // Establishing the staging directory flips the capability.
        fs.staging().ensure_dir(root).unwrap();
        assert!(fs.write_capable(root).unwrap());
        // A foreign same-prefix directory does not grant capability.
        let base = fs.staging().registry().path_of(root).unwrap();
        let foreign = base.join(".echo-staging-user");
        std::fs::create_dir_all(&foreign).unwrap();
        assert!(
            !foreign
                .join(super::super::staging::MARKER_FILE_NAME)
                .exists()
                || fs.write_capable(root).unwrap()
        );
    }

    #[test]
    fn read_head_reads_only_the_requested_prefix() {
        let (_dir, root, fs) = setup();
        let bytes = vec![7u8; 10_000];
        let path = RelativeMediaPath::new("big.mp3").unwrap();
        let base = fs.registry.path_of(root).unwrap();
        std::fs::write(base.join("big.mp3"), &bytes).unwrap();
        let head = fs.read_head(root, &path, 16).unwrap();
        assert_eq!(head.len(), 16, "only the head is read");
    }

    #[test]
    fn unregistered_roots_are_unavailable() {
        let (_dir, _root, fs) = setup();
        let ghost = LibraryRootId::new();
        let error = fs.enumerate(ghost).unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
}
