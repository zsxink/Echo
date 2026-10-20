//! The root-constrained [`LibraryFileSystem`] adapter (task 4.2) with the
//! task-5.3 ingestion mechanics: streaming copies into the marker-verified
//! staging directory, fsync before a staged copy counts, exclusive target
//! reservation and per-file atomic publish.

use std::collections::HashMap;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::application::ports::filesystem::LegacyLibraryFileSystem;
use crate::application::ports::{FileMeta, StagedCopy, StagedResource};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::error::{Error, PermKind};

use super::registry::RootRegistry;
use super::staging::{StagingCheck, StagingManager};
use super::walker;

/// Chunk size of the streaming copy (bounded memory: a multi-gigabyte source
/// never buffers whole in RAM).
const COPY_CHUNK_BYTES: usize = 64 * 1024;
// Design §9: deletions stage into `trash/<operation-id>` inside the same
/// controlled staging directory (同盘 rename, recovery-safe and skipped by the
/// scanner because the whole staging directory is marker-verified).
const TRASH_STAGING_SUBDIR: &str = "trash";
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
        self.registry.resolve_relative(root, path)
    }

    /// The adapter-private staging slot of one operation resource:
    /// `<echo/tmp>/<operation-id>/<resource>` (portable-layout spec:
    /// `tmp/<operation-id>`).
    fn slot(staging_dir: &Path, staged: &StagedResource) -> PathBuf {
        staging_dir.join(staged.operation().as_uuid().simple().to_string())
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
        let staged_path = RelativeMediaPath::new(&format!(
            "{}/{}/{}",
            crate::domain::library::STAGING_ROOT,
            staged.operation().as_uuid().simple(),
            staged.resource_key()
        ))?;
        self.registry.resolve_relative(root, &staged_path)?;
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

        let (size, blake3) = copy_staging_content(content, &mut out, &part)?;
        out.flush()
            .map_err(|source| Error::io("flush staging copy", source, part.clone()))?;
        // fsync: the staged copy is only "staged" once it survives a crash.
        out.sync_all()
            .map_err(|source| Error::io("fsync staging copy", source, part.clone()))?;
        drop(out);
        std::fs::rename(&part, &file)
            .map_err(|source| Error::io("finalize staging copy", source, file.clone()))?;
        fsync_dir(&slot);

        // The journal wants the staged location relative to the root; for the
        // portable layout this is always `echo/tmp/<operation>/<resource>`.
        self.staged.lock().unwrap().insert(
            (root, staged.operation(), staged.resource_key().to_owned()),
            StagedLocation { staging_dir, file },
        );
        Ok(StagedCopy {
            size,
            blake3,
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
        self.registry.resolve_relative(
            root,
            &RelativeMediaPath::new(&format!(
                "{}/{}/{}",
                crate::domain::library::STAGING_ROOT,
                staged.operation().as_uuid().simple(),
                staged.resource_key()
            ))?,
        )?;
        Ok(location)
    }
}

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for RootConstrainedFileSystem {
    fn is_publication_blocked(&self, error: &Error) -> bool {
        // These native codes mean sharing/lock violations only on Windows.
        // The same numbers describe unrelated errors on Unix platforms.
        std::env::consts::OS == "windows"
            && matches!(error, Error::Io { source, .. } if matches!(source.raw_os_error(), Some(32 | 33)))
    }
    fn retain_trash_resource(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        expected_hash: &str,
    ) -> Result<crate::application::ports::RetainedResource, Error> {
        self.verify_trash_path(root, path)?;
        super::retained::copy(&self.registry, root, path, expected_hash)
    }
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
            if self.resolve_verified(root, staged).is_ok() {
                let _ = std::fs::remove_file(&location.file);
            }
            self.staged.lock().unwrap().remove(&key);
        }
        Ok(())
    }

    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
        let abs = self.abs(root, path)?;
        match abs.symlink_metadata() {
            Ok(_) => Ok(true),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(Error::io("inspect library path", source, abs)),
        }
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

    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let abs = self.abs(root, target)?;
        // Never follow or remove a symlink/reparse point (task 4.2 boundary):
        // only a real regular file published into the library is removed.
        let Ok(meta) = std::fs::symlink_metadata(&abs) else {
            return Ok(()); // absent: idempotent
        };
        if meta.file_type().is_symlink() {
            return Err(Error::permission("discard published", PermKind::NotOwner));
        }
        if meta.is_file() {
            std::fs::remove_file(&abs)
                .map_err(|source| Error::io("discard published", source, abs))?;
        }
        Ok(())
    }

    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        // Portable layout: deletes stage into `echo/tmp/trash/<operation>/<key>`.
        let staging_dir = self.staging.ensure_dir(root)?;
        let root_abs = self.registry.path_of(root)?;
        let trash_rel = RelativeMediaPath::new(&format!(
            "{}/{TRASH_STAGING_SUBDIR}/{}/{}",
            staging_dir
                .strip_prefix(&root_abs)
                .map_err(|source| {
                    Error::io(
                        "resolve trash slot",
                        std::io::Error::other(source),
                        staging_dir.clone(),
                    )
                })?
                .to_string_lossy()
                .replace('\\', "/"),
            operation.as_uuid().simple(),
            resource_key
        ))?;
        let _ = self.verify_trash_path(root, &trash_rel)?;
        Ok(trash_rel)
    }

    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        let source_abs = self.abs(root, source)?;
        if self.owned_staging_ancestor(&source_abs, root) {
            return Err(Error::permission("stage to trash", PermKind::NotOwner));
        }
        // The source must be a real regular library file (never a symlink,
        // never a directory); the move is a same-volume rename.
        match std::fs::symlink_metadata(&source_abs) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::permission("stage to trash", PermKind::NotOwner));
            }
            _ => {
                return Err(Error::io(
                    "stat source",
                    std::io::Error::other("missing"),
                    source_abs,
                ))
            }
        }
        let trash_rel = self.trash_path(root, operation, resource_key)?;
        let trash_abs = self.verify_trash_path(root, &trash_rel)?;
        if let Some(parent) = trash_abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| Error::io("create trash slot", source, parent.to_path_buf()))?;
        }
        // Exclusive-create the trash slot: never overwrite an existing entry.
        let reserved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&trash_abs);
        match reserved {
            Ok(handle) => {
                if let Err(source) = handle.sync_all() {
                    return Err(Error::io("fsync reserved trash target", source, trash_abs));
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::conflict("trash target already exists"));
            }
            Err(source) => return Err(Error::io("reserve trash target", source, trash_abs)),
        }
        if let Err(source) = std::fs::rename(&source_abs, &trash_abs) {
            let still_empty = std::fs::metadata(&trash_abs).is_ok_and(|meta| meta.len() == 0);
            if still_empty {
                let _ = std::fs::remove_file(&trash_abs);
            }
            return Err(Error::io("stage to trash", source, trash_abs));
        }
        fsync_dir(
            &trash_abs
                .parent()
                .map_or_else(|| trash_abs.clone(), std::path::Path::to_path_buf),
        );
        Ok(trash_rel)
    }

    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let trash_abs = self.verify_trash_path(root, trash)?;
        let dest = self.abs(root, target)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io("create target directory", source, parent.to_path_buf())
            })?;
        }
        // Same exclusive create-new + fsync + rename contract: the restore
        // never replaces foreign content; a zero-byte placeholder left by a
        // crashed earlier attempt is cleared first.
        if let Ok(meta) = std::fs::metadata(&dest) {
            if meta.len() == 0 {
                let _ = std::fs::remove_file(&dest);
            }
        }
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
                return Err(Error::conflict("restore target file already exists"));
            }
            Err(source) => return Err(Error::io("reserve target", source, dest)),
        }
        if let Err(source) = std::fs::rename(&trash_abs, &dest) {
            let still_empty = std::fs::metadata(&dest).is_ok_and(|meta| meta.len() == 0);
            if still_empty {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(Error::io("restore from trash", source, dest));
        }
        fsync_dir(
            &dest
                .parent()
                .map_or_else(|| dest.clone(), std::path::Path::to_path_buf),
        );
        Ok(())
    }

    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.staging.write_capable(root)
    }

    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
        // Idempotent exclusive-create of the owned staging dir (design §8).
        self.staging.ensure_dir(root).map(|_| ())
    }
}

impl RootConstrainedFileSystem {
    /// Whether `path` resolves inside a marker-verified Echo staging directory
    /// of `root` (walking up from the file to the owned `echo/tmp` ancestor).
    fn owned_staging_ancestor(&self, path: &Path, root: LibraryRootId) -> bool {
        self.staging.owned_staging_ancestor(path, root)
    }

    /// Verify a `trash/*` root-relative path resolves inside the owned staging
    /// directory directly under a `trash` subdirectory (design §9: 专属受控
    /// `trash/<operation-id>`). Only Echo's own marker-verified trash slot is
    /// ever read or written — a crafted/foreign path is refused.
    fn verify_trash_path(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
    ) -> Result<PathBuf, Error> {
        let abs = self.abs(root, trash)?;
        let Some(ancestor) = self.staging.staging_root(root) else {
            return Err(Error::permission("trash path", PermKind::NotOwner));
        };
        if self.staging.check(&ancestor, root) != super::staging::StagingCheck::Owned {
            return Err(Error::permission("trash path", PermKind::NotOwner));
        }
        // The component immediately below the staging dir must be `trash`.
        let under = abs
            .strip_prefix(&ancestor)
            .map_err(|_| Error::permission("trash path", PermKind::NotOwner))?;
        let first = under
            .components()
            .next()
            .and_then(|c| match c {
                std::path::Component::Normal(segment) => {
                    Some(segment.to_string_lossy().into_owned())
                }
                _ => None,
            })
            .ok_or_else(|| Error::permission("trash path", PermKind::NotOwner))?;
        if first != TRASH_STAGING_SUBDIR || under.components().count() < 2 {
            return Err(Error::permission("trash path", PermKind::NotOwner));
        }
        Ok(abs)
    }
}

fn copy_staging_content(
    content: &mut dyn Read,
    out: &mut dyn std::io::Write,
    part: &Path,
) -> Result<(u64, String), Error> {
    let mut hasher = blake3::Hasher::new();
    let mut size: u64 = 0;
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
    loop {
        let read = content
            .read(&mut buffer)
            .map_err(|source| Error::io("copy import source", source, part))?;
        if read == 0 {
            return Ok((size, hasher.finalize().to_hex().to_string()));
        }
        hasher.update(&buffer[..read]);
        out.write_all(&buffer[..read])
            .map_err(|source| Error::io("write staging copy", source, part))?;
        size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
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
mod tests;
