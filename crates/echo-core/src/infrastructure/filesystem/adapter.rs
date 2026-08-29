//! The root-constrained [`LibraryFileSystem`] adapter (task 4.2) with the
//! task-5.3 ingestion mechanics: streaming copies into the marker-verified
//! staging directory, fsync before a staged copy counts, exclusive target
//! reservation and per-file atomic publish.

use std::collections::HashMap;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::application::ports::{FileMeta, LibraryFileSystem, StagedCopy, StagedResource};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::error::{Error, PermKind};

use super::registry::RootRegistry;
use super::staging::{StagingCheck, StagingManager};
use super::walker;

/// Chunk size of the streaming copy (bounded memory: a multi-gigabyte source
/// never buffers whole in RAM).
const COPY_CHUNK_BYTES: usize = 64 * 1024;
/// Design §8: imports stage under `import/<operation-id>` inside the
/// controlled staging directory (deletions later use `trash/<operation-id>`).
const IMPORT_STAGING_SUBDIR: &str = "import";
/// Suffix of the exclusive in-progress staging file; only a completed copy is
/// renamed onto the final staged name, so a partial file can never pass for a
/// staged resource.
const PART_SUFFIX: &str = ".part";

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

    /// The adapter-private staging slot of one operation resource:
    /// `<staging>/import/<operation>/<resource>` (design §8).
    fn slot(staging_dir: &Path, staged: &StagedResource) -> PathBuf {
        staging_dir
            .join(IMPORT_STAGING_SUBDIR)
            .join(staged.operation().as_uuid().simple().to_string())
    }

    /// Write bytes into a fresh file under the root's staging directory and
    /// register the handle. Shared helper behind the port's [`Self::stage`]
    /// (test setup entry); the streaming [`Self::stage_stream`] is the
    /// ingestion path the import drives.
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
        self.stage_stream(root, staged, &mut std::io::Cursor::new(bytes))
            .map(|_| ())
    }

    /// The streaming staging path (task 5.3): exclusive-create a `.part`
    /// file inside the owned staging slot, pump the source through it in
    /// bounded chunks while accumulating BLAKE3, fsync the completed copy and
    /// only then rename it onto the final staged name (atomic within the
    /// staging directory). A `.part` file can only be Echo's own leftover
    /// from a crashed attempt of the *same* operation in the *same*
    /// marker-verified directory, so clearing it before the exclusive
    /// re-create never touches foreign content.
    fn stage_stream_inner(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error> {
        let staging_dir = self.staging.ensure_dir(root)?;
        let slot = Self::slot(&staging_dir, staged);
        std::fs::create_dir_all(&slot)
            .map_err(|source| Error::io("create staging slot", source, slot.clone()))?;
        let file = slot.join(staged.resource_key());
        let part = slot.join(format!("{}{}", staged.resource_key(), PART_SUFFIX));
        let _ = std::fs::remove_file(&part);
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|source| Error::io("create staging file", source, part.clone()))?;

        let mut hasher = blake3::Hasher::new();
        let mut size: u64 = 0;
        let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
        loop {
            let read = content
                .read(&mut buffer)
                .map_err(|source| Error::io("copy import source", source, part.clone()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])
                .map_err(|source| Error::io("write staging copy", source, part.clone()))?;
            size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        out.flush()
            .map_err(|source| Error::io("flush staging copy", source, part.clone()))?;
        // fsync: the staged copy is only "staged" once it survives a crash.
        out.sync_all()
            .map_err(|source| Error::io("fsync staging copy", source, part.clone()))?;
        drop(out);
        std::fs::rename(&part, &file)
            .map_err(|source| Error::io("finalize staging copy", source, file.clone()))?;
        fsync_dir(&slot);

        // The journal wants the staged location relative to the root; the
        // staging directory name is stable (marker-verified reuse).
        let root_abs = self.registry.path_of(root)?;
        let staging_name = match staging_dir.strip_prefix(&root_abs) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(source) => {
                return Err(Error::io(
                    "resolve staging location",
                    std::io::Error::other(source),
                    staging_dir.clone(),
                ))
            }
        };
        let staged_path = RelativeMediaPath::new(&format!(
            "{staging_name}/{IMPORT_STAGING_SUBDIR}/{}/{}",
            staged.operation().as_uuid().simple(),
            staged.resource_key()
        ))?;
        self.staged.lock().unwrap().insert(
            (root, staged.operation(), staged.resource_key().to_owned()),
            StagedLocation { staging_dir, file },
        );
        Ok(StagedCopy {
            size,
            blake3: hasher.finalize().to_hex().to_string(),
            staged_path,
        })
    }

    /// Resolve a staged handle to its location, refusing unknown handles and
    /// directories that lost their marker between staging and use.
    fn resolve_verified(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
    ) -> Result<StagedLocation, Error> {
        let key = (root, staged.operation(), staged.resource_key().to_owned());
        let location = self
            .staged
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::permission("resolve staged resource", PermKind::NotOwner))?;
        // The staging directory must still be marker-verified at use time; a
        // directory that lost its marker between staging and use is no longer
        // trusted.
        if self.staging.check(&location.staging_dir, root) != StagingCheck::Owned {
            return Err(Error::permission(
                "verify staging marker",
                PermKind::NotOwner,
            ));
        }
        Ok(location)
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
        let location = self.resolve_verified(root, staged)?;
        let dest = self.abs(root, target)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io("create target directory", source, parent.to_path_buf())
            })?;
        }
        // Exclusive target reservation (design §8: 新建 exclusive 目标 + fsync
        // + rename，绝不替换): the create-new fails on ANY existing entry — a
        // user file, a directory or a symlink (O_EXCL does not follow links) —
        // so Echo can never take over or replace foreign content.
        let reserved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest);
        match reserved {
            Ok(handle) => {
                if let Err(source) = handle.sync_all() {
                    return Err(Error::io("fsync reserved target", source, dest));
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::conflict("target file already exists"));
            }
            Err(source) => return Err(Error::io("reserve target", source, dest)),
        }
        // The rename swaps our own empty reservation for the verified staged
        // content — the file only ever appears whole, never half-written.
        if let Err(source) = std::fs::rename(&location.file, &dest) {
            // Clear the placeholder this call created (only while it is still
            // the empty file we own) so a retry starts clean.
            let still_empty = std::fs::metadata(&dest).is_ok_and(|meta| meta.len() == 0);
            if still_empty {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(Error::io("publish", source, dest));
        }
        fsync_dir(
            &dest
                .parent()
                .map_or_else(|| dest.clone(), std::path::Path::to_path_buf),
        );
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

    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error> {
        self.stage_stream_inner(root, staged, content)
    }

    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error> {
        let location = self.resolve_verified(root, staged)?;
        std::fs::read(&location.file)
            .map_err(|source| Error::io("read staged copy", source, location.file))
    }

    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error> {
        let key = (root, staged.operation(), staged.resource_key().to_owned());
        let location = self.staged.lock().unwrap().get(&key).cloned();
        if let Some(location) = location {
            // Only remove inside a still marker-verified staging directory:
            // cleanup must never reach beyond Echo's own slot.
            if self.staging.check(&location.staging_dir, root) == StagingCheck::Owned {
                let _ = std::fs::remove_file(&location.file);
            }
            self.staged.lock().unwrap().remove(&key);
        }
        Ok(())
    }

    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
        let abs = self.abs(root, path)?;
        Ok(abs.symlink_metadata().is_ok())
    }

    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let staging_abs = self.abs(root, staging_path)?;
        // The journal's persisted staging path must resolve inside Echo's own
        // marker-verified staging directory; a directory that lost its marker
        // (or was never ours) is refused, so a crafted or foreign path can
        // never be published.
        if !self.owned_staging_ancestor(&staging_abs, root) {
            return Err(Error::permission(
                "publish from staging path",
                PermKind::NotOwner,
            ));
        }
        let dest = self.abs(root, target)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io("create target directory", source, parent.to_path_buf())
            })?;
        }
        // A crash between the exclusive reserve and the rename leaves our own
        // zero-byte placeholder at the target (the same stub `publish` clears
        // on a rename failure). A non-empty file is never touched.
        if let Ok(meta) = std::fs::metadata(&dest) {
            if meta.len() == 0 {
                let _ = std::fs::remove_file(&dest);
            }
        }
        // Same exclusive create-new + fsync + rename contract as `publish`
        // (design §8: 新建 exclusive 目标 + fsync + rename，绝不替换).
        let reserved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest);
        match reserved {
            Ok(handle) => {
                if let Err(source) = handle.sync_all() {
                    return Err(Error::io("fsync reserved target", source, dest));
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::conflict("target file already exists"));
            }
            Err(source) => return Err(Error::io("reserve target", source, dest)),
        }
        if let Err(source) = std::fs::rename(&staging_abs, &dest) {
            let still_empty = std::fs::metadata(&dest).is_ok_and(|meta| meta.len() == 0);
            if still_empty {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(Error::io("publish", source, dest));
        }
        fsync_dir(
            &dest
                .parent()
                .map_or_else(|| dest.clone(), std::path::Path::to_path_buf),
        );
        Ok(())
    }

    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let staging_abs = self.abs(root, staging_path)?;
        if !self.owned_staging_ancestor(&staging_abs, root) {
            return Err(Error::permission(
                "discard staging path",
                PermKind::NotOwner,
            ));
        }
        let _ = std::fs::remove_file(&staging_abs);
        Ok(())
    }

    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.staging.write_capable(root)
    }
}

impl RootConstrainedFileSystem {
    /// Whether `path` resolves inside a marker-verified Echo staging directory
    /// of `root` (walking up from the file to its `.echo-staging-*` ancestor).
    fn owned_staging_ancestor(&self, path: &Path, root: LibraryRootId) -> bool {
        let Some(ancestor) = path.ancestors().find(|p| {
            p.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with(super::staging::STAGING_DIR_PREFIX)
            })
        }) else {
            return false;
        };
        self.staging.check(ancestor, root) == super::staging::StagingCheck::Owned
    }
}

/// Best-effort directory fsync (rename durability). Some platforms cannot
/// open directories at all; the file-level fsyncs above carry the hard
/// requirement, the directory flush is advisory and never fails a publish.
fn fsync_dir(path: &Path) {
    match std::fs::File::open(path) {
        Ok(handle) => {
            if let Err(source) = handle.sync_all() {
                tracing::debug!(directory = %path.display(), %source, "directory fsync unavailable");
            }
        }
        Err(source) => {
            tracing::debug!(directory = %path.display(), %source, "directory open for fsync failed");
        }
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

    // -----------------------------------------------------------------------
    // Task 5.3: streaming staging + BLAKE3, fsync, exclusive publish and
    // staging-slot hygiene, verified against the real filesystem in temp dirs.
    // -----------------------------------------------------------------------

    #[test]
    fn staged_copy_streams_hashes_and_lands_in_the_owned_import_slot() {
        let (dir, root, fs) = setup();
        let operation = OperationId::new();
        let staged = StagedResource::new(operation, "audio").unwrap();
        // Multi-chunk content: the copy must pump it through in bounded
        // pieces, not buffer the whole file (chunk size is 64 KiB).
        let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut reader: &[u8] = &content;

        let copy = fs.stage_stream(root, &staged, &mut reader).unwrap();
        assert_eq!(copy.size, u64::try_from(content.len()).unwrap());
        assert_eq!(copy.blake3, blake3::hash(&content).to_hex().to_string());
        // The staged slot follows design §8: `<staging>/import/<operation>`.
        assert!(
            copy.staged_path
                .display()
                .split('/')
                .next()
                .is_some_and(|first| first.starts_with(".echo-staging-")),
            "staged under the controlled directory: {}",
            copy.staged_path.display()
        );
        assert!(copy.staged_path.display().contains("/import/"));
        // The staged file physically exists under the root and round-trips.
        let staged_abs = dir.path().join(copy.staged_path.normalized());
        assert_eq!(std::fs::read(&staged_abs).unwrap(), content);
        assert_eq!(fs.read_staged(root, &staged).unwrap(), content);
        // No half-written `.part` residue after the atomic finalize.
        assert!(
            !Path::new(&format!("{}.part", staged_abs.display())).exists(),
            "the exclusive part file was renamed away"
        );

        // Publish moves the staged copy out; the target holds every byte.
        let target = RelativeMediaPath::new("歌手/歌手 - 大文件.flac").unwrap();
        fs.publish(root, &staged, &target).unwrap();
        assert_eq!(
            fs.read_head(root, &target, u64::MAX).unwrap(),
            content,
            "the full audio is published whole"
        );
        assert!(fs.read_staged(root, &staged).is_err(), "staged is gone");
    }

    #[test]
    fn publish_reserves_the_target_exclusively_and_never_replaces() {
        let (dir, root, fs) = setup();
        let target = RelativeMediaPath::new("歌手/歌手 - 晴天.flac").unwrap();
        let base = dir.path();

        // First publish wins the exclusive reservation.
        let first = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(root, &first, b"winner-bytes").unwrap();
        fs.publish(root, &first, &target).unwrap();

        // A second publish onto the occupied name is refused, and the
        // incumbent content is untouched (绝不覆盖既有文件).
        let second = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(root, &second, b"loser-bytes").unwrap();
        let error = fs.publish(root, &second, &target).unwrap_err();
        assert_eq!(error.code(), "conflict");
        assert_eq!(
            fs.read_head(root, &target, u64::MAX).unwrap(),
            b"winner-bytes"
        );
        // The failed publish cleared its own (empty) reservation, so a retry
        // starts clean.
        assert_eq!(
            std::fs::read(base.join(target.normalized())).unwrap(),
            b"winner-bytes"
        );
        fs.discard_staged(root, &second).unwrap();

        // A symlink sitting at the destination is never followed or replaced:
        // the exclusive create-new refuses the name outright.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("precious.txt"), b"precious").unwrap();
        let link_target = RelativeMediaPath::new("linked.flac").unwrap();
        create_symlink(
            &outside.path().join("precious.txt"),
            &base.join("linked.flac"),
        );
        let third = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(root, &third, b"symlink-victim").unwrap();
        let error = fs.publish(root, &third, &link_target).unwrap_err();
        assert_eq!(error.code(), "conflict", "symlink at target is a conflict");
        assert_eq!(
            std::fs::read(outside.path().join("precious.txt")).unwrap(),
            b"precious",
            "the symlink target was never touched"
        );
    }

    /// Platform-neutral symlink creation for tests (runtime helper choice, no
    /// platform-conditional code — see the staging tests).
    fn create_symlink(target: &Path, link: &Path) {
        let status = if std::env::consts::OS == "windows" {
            std::process::Command::new("cmd")
                .args(["/C", "mklink"])
                .arg(link)
                .arg(target)
                .status()
                .expect("spawn cmd")
        } else {
            std::process::Command::new("ln")
                .arg("-s")
                .arg(target)
                .arg(link)
                .status()
                .expect("spawn ln")
        };
        assert!(status.success(), "symlink creation must succeed");
    }

    #[test]
    fn discard_staged_is_idempotent_and_never_reaches_outside_the_owned_slot() {
        let (_dir, root, fs) = setup();
        let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(root, &staged, b"to-discard").unwrap();
        fs.discard_staged(root, &staged).unwrap();
        assert!(fs.read_staged(root, &staged).is_err());
        // Idempotent: discarding again (and discarding an unknown handle)
        // succeeds without effect.
        fs.discard_staged(root, &staged).unwrap();
        fs.discard_staged(
            root,
            &StagedResource::new(OperationId::new(), "audio").unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn user_directory_with_a_staging_name_is_never_written() {
        let (dir, root, fs) = setup();
        // A user directory that happens to carry Echo's staging prefix, with
        // real user content inside.
        let foreign = dir.path().join(".echo-staging-user-owned");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("user-file.txt"), b"user-content").unwrap();

        // A full ingest cycle must pick its own random directory and leave
        // the foreign one completely alone.
        let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(root, &staged, b"import-bytes").unwrap();
        let target = RelativeMediaPath::new("歌手/歌手 - 晴天.flac").unwrap();
        fs.publish(root, &staged, &target).unwrap();

        assert_eq!(
            std::fs::read(foreign.join("user-file.txt")).unwrap(),
            b"user-content",
            "user file content unchanged"
        );
        assert_eq!(
            std::fs::read_dir(&foreign).unwrap().count(),
            1,
            "nothing was written into the user directory"
        );
        // Echo's writes landed in its own marker-verified directory instead.
        let owned = fs.staging().ensure_dir(root).unwrap();
        assert_ne!(owned, foreign);
        assert!(fs.write_capable(root).unwrap());
    }
}
