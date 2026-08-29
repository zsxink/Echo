//! Root-constrained [`LibraryFileSystem`] fake backed by a real temp dir.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::unnecessary_to_owned,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::let_and_return,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::unchecked_time_subtraction,
    clippy::wildcard_imports,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::application::ports::*;
use crate::domain::ids::*;
use crate::error::Error;

/// Shared interior-mutability cell backing every in-memory fake.
type Shared<T> = Arc<Mutex<T>>;

/// A root-constrained file system backed by a real temp directory, with
/// scriptable faults. `tempfile` guarantees the path never touches the user's
/// home.
#[derive(Clone, Debug)]
pub struct FakeLibraryFileSystem {
    /// root_id → absolute temp dir
    roots: Shared<BTreeMap<LibraryRootId, PathBuf>>,
    /// scriptable read/write failure injection (a code + message; `Error` is
    /// not `Clone`, so we store a cheap equivalent)
    fault: Shared<Option<(String, String)>>,
    write_capable: Shared<bool>,
    /// Adapter-private staged files keyed by typed operation resource handle.
    staged: Shared<BTreeMap<(LibraryRootId, OperationId, String), PathBuf>>,
}

impl FakeLibraryFileSystem {
    /// Create a temp-dir-backed fake, registering `root` at a fresh temp dir.
    pub fn with_root(root: LibraryRootId) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.keep();
        let this = Self {
            roots: Arc::new(Mutex::new(BTreeMap::from([(root, path)]))),
            fault: Arc::new(Mutex::new(None)),
            write_capable: Arc::new(Mutex::new(true)),
            staged: Arc::new(Mutex::new(BTreeMap::new())),
        };
        this
    }

    /// Make the next operation fail (simulates permission revocation / IO).
    /// The stored fault is rebuilt into an owned [`Error`] on read.
    pub fn inject_fault(&self, err: Error) {
        *self.fault.lock().unwrap() = Some((err.code().to_owned(), err.to_string()));
    }

    /// Stage bytes inside the fake's owned root. This is test setup only; the
    /// public Port exposes only [`StagedResource`], never an arbitrary path.
    pub fn stage_bytes(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        bytes: &[u8],
    ) -> Result<(), Error> {
        self.stage_stream(root, staged, &mut std::io::Cursor::new(bytes))
            .map(|_| ())
    }

    /// How many staged resources are currently registered (assertion helper:
    /// the import's failure paths must discard their staged copies).
    #[must_use]
    pub fn staged_count(&self) -> usize {
        self.staged.lock().unwrap().len()
    }

    pub fn clear_fault(&self) {
        *self.fault.lock().unwrap() = None;
    }

    /// The absolute temp-dir path of a registered root (test setup only).
    #[must_use]
    pub fn root_path(&self, root: LibraryRootId) -> Option<PathBuf> {
        self.roots.lock().unwrap().get(&root).cloned()
    }

    /// Register an *additional* root at its own fresh temp dir (multi-root
    /// scenarios such as root switching).
    pub fn add_root(&self, root: LibraryRootId) -> PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.keep();
        self.roots.lock().unwrap().insert(root, path.clone());
        path
    }

    /// Register an additional root at an explicit path (root-switch tests
    /// point the candidate at its own temp directory).
    pub fn add_root_at(&self, root: LibraryRootId, path: PathBuf) {
        self.roots.lock().unwrap().insert(root, path);
    }
    /// Programmatically toggle write capability.
    pub fn set_write_capable(&self, capable: bool) {
        *self.write_capable.lock().unwrap() = capable;
    }

    fn abs(&self, root: LibraryRootId, rel: &RelativeMediaPath) -> PathBuf {
        self.roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .expect("unknown root")
            .join(rel.normalized())
    }

    /// Rebuild an owned [`Error`] from the scripted fault (tests only assert
    /// failure, never the exact variant shape).
    fn fault_error(&self) -> Option<Error> {
        self.fault
            .lock()
            .unwrap()
            .as_ref()
            .map(|(what, msg)| Error::Storage {
                what: what.clone(),
                source: std::io::Error::other(msg.clone()).into(),
            })
    }
}

impl LibraryFileSystem for FakeLibraryFileSystem {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .expect("root");
        let mut out = Vec::new();
        walk_dir(&base, &base, &mut out);
        Ok(out)
    }
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let m = std::fs::metadata(self.abs(root, path))
            .map_err(|e| Error::io("stat", e, self.abs(root, path)))?;
        Ok(FileMeta {
            size: m.len(),
            modified_ns: m
                .modified()
                .ok()
                .map(|t| {
                    t.duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as i64
                })
                .unwrap_or(0),
        })
    }
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let abs = self.abs(root, path);
        let data = std::fs::read(&abs).map_err(|e| Error::io("read", e, abs.clone()))?;
        Ok(data.into_iter().take(limit as usize).collect())
    }
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let key = (root, staged.operation(), staged.resource_key().to_owned());
        let staged_path = self
            .staged
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::permission("publish", crate::error::PermKind::NotOwner))?;
        let dest = self.abs(root, target);
        if dest.exists() {
            // Publish is create-new (design §8): never replace existing
            // content. The fake mirrors the real adapter so use-case tests can
            // assert "绝不覆盖既有文件" through the port.
            return Err(Error::conflict("target file already exists"));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io("create_dir_all", e, parent.to_path_buf()))?;
        }
        std::fs::rename(&staged_path, &dest).map_err(|e| Error::io("rename", e, dest.clone()))?;
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

    /// Mirrors the real adapter: chunked pump + BLAKE3 accumulated during the
    /// copy, staged under `<root>/.echo-test-staging/import/<operation>/`.
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn std::io::Read,
    ) -> Result<StagedCopy, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .ok_or_else(|| Error::unavailable("test root", "unknown root"))?;
        let slot = base
            .join(".echo-test-staging")
            .join("import")
            .join(staged.operation().to_string());
        std::fs::create_dir_all(&slot).map_err(|e| Error::io("stage mkdir", e, slot.clone()))?;
        let path = slot.join(staged.resource_key());
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| Error::io("stage open", e, path.clone()))?;
        let mut hasher = blake3::Hasher::new();
        let mut size: u64 = 0;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = content
                .read(&mut buffer)
                .map_err(|e| Error::io("copy import source", e, path.clone()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|e| Error::io("stage write", e, path.clone()))?;
            size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        let staged_rel = RelativeMediaPath::new(&format!(
            ".echo-test-staging/import/{}/{}",
            staged.operation(),
            staged.resource_key()
        ))?;
        self.staged.lock().unwrap().insert(
            (root, staged.operation(), staged.resource_key().to_owned()),
            path.clone(),
        );
        Ok(StagedCopy {
            size,
            blake3: hasher.finalize().to_hex().to_string(),
            staged_path: staged_rel,
        })
    }

    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let path = self
            .staged
            .lock()
            .unwrap()
            .get(&(root, staged.operation(), staged.resource_key().to_owned()))
            .cloned()
            .ok_or_else(|| Error::permission("read staged", crate::error::PermKind::NotOwner))?;
        std::fs::read(&path).map_err(|e| Error::io("read staged copy", e, path))
    }

    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error> {
        let key = (root, staged.operation(), staged.resource_key().to_owned());
        // Bound the clone first so the guard is gone before the removal lock.
        let path = self.staged.lock().unwrap().get(&key).cloned();
        if let Some(path) = path {
            let _ = std::fs::remove_file(&path);
            self.staged.lock().unwrap().remove(&key);
        }
        Ok(())
    }

    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        Ok(self.abs(root, path).exists())
    }

    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        // Only Echo's own staging area may be republished after a crash: the
        // persisted staging path must live under `<root>/.echo-test-staging/`.
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .ok_or_else(|| Error::unavailable("test root", "unknown root"))?;
        // The journal's staging path is root-relative; resolve it and verify
        // it sits directly under the root's `.echo-test-staging` directory
        // (never a foreign location). Component-wise check, not a string
        // prefix, so a crafted name cannot evade the boundary.
        let staging_abs = base.join(staging_path.normalized());
        let staging_root = base.join(".echo-test-staging");
        let inside = staging_abs
            .strip_prefix(&staging_root)
            .is_ok_and(|rest| !rest.as_os_str().is_empty() && !rest.starts_with(".."));
        if !inside {
            return Err(Error::permission(
                "publish from staging path",
                crate::error::PermKind::NotOwner,
            ));
        }
        let dest = self.abs(root, target);
        // A crash between exclusive reserve and rename leaves our own empty
        // placeholder; only a zero-byte target is cleared (never foreign
        // content).
        if let Ok(meta) = std::fs::metadata(&dest) {
            if meta.len() == 0 {
                let _ = std::fs::remove_file(&dest);
            }
        }
        if dest.exists() {
            return Err(Error::conflict("target file already exists"));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io("create_dir_all", e, parent.to_path_buf()))?;
        }
        std::fs::rename(&staging_abs, &dest).map_err(|e| Error::io("rename", e, dest.clone()))?;
        // If the handle was (still) registered, drop it so later discards are
        // clean no-ops.
        self.staged
            .lock()
            .unwrap()
            .retain(|_key, path| path.clone() != staging_abs);
        Ok(())
    }

    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .ok_or_else(|| Error::unavailable("test root", "unknown root"))?;
        let staging_abs = base.join(staging_path.normalized());
        let staging_root = base.join(".echo-test-staging");
        let inside = staging_abs
            .strip_prefix(&staging_root)
            .is_ok_and(|rest| !rest.as_os_str().is_empty() && !rest.starts_with(".."));
        if !inside {
            return Err(Error::permission(
                "discard staging path",
                crate::error::PermKind::NotOwner,
            ));
        }
        let _ = std::fs::remove_file(&staging_abs);
        self.staged
            .lock()
            .unwrap()
            .retain(|_key, path| path.clone() != staging_abs);
        Ok(())
    }

    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        let _ = root;
        Ok(*self.write_capable.lock().unwrap())
    }
}

fn walk_dir(base: &Path, dir: &Path, out: &mut Vec<RelativeMediaPath>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                // The fake's private staging area is invisible to scans, like
                // the real walker's marker-verified skip.
                if p.file_name()
                    .is_some_and(|name| name == ".echo-test-staging")
                {
                    continue;
                }
                walk_dir(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                if let Ok(rp) = RelativeMediaPath::new(&rel.to_string_lossy()) {
                    out.push(rp);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> LibraryRootId {
        LibraryRootId::new()
    }

    #[test]
    fn fake_fs_simulates_permission_revocation_and_publish_works() {
        let r = root();
        let fs = FakeLibraryFileSystem::with_root(r);
        // Only a typed, adapter-owned staging handle can be published.
        let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
        fs.stage_bytes(r, &staged, b"audio").unwrap();
        fs.publish(
            r,
            &staged,
            &RelativeMediaPath::new("华语/稻香.mp3").unwrap(),
        )
        .unwrap();
        assert!(fs
            .roots
            .lock()
            .unwrap()
            .get(&r)
            .unwrap()
            .join("华语/稻香.mp3")
            .exists());
        // Enumerate sees it.
        let found = fs.enumerate(r).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].display(), "华语/稻香.mp3");

        // Fault injection simulates permission revocation.
        fs.inject_fault(Error::unavailable("test root", "权限被撤销"));
        assert!(fs.enumerate(r).is_err());
        assert!(fs
            .read_head(r, &RelativeMediaPath::new("华语/稻香.mp3").unwrap(), 4)
            .is_err());
        fs.clear_fault();
        assert!(fs.enumerate(r).is_ok());

        let forged = StagedResource::new(OperationId::new(), "forged").unwrap();
        let err = fs
            .publish(r, &forged, &RelativeMediaPath::new("x.mp3").unwrap())
            .unwrap_err();
        assert_eq!(err.code(), "permission");
    }
}
