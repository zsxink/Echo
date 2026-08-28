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
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .ok_or_else(|| Error::unavailable("test root", "unknown root"))?;
        let path = base
            .join(".echo-test-staging")
            .join(staged.operation().to_string())
            .join(staged.resource_key());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("stage mkdir", e, parent))?;
        }
        std::fs::write(&path, bytes).map_err(|e| Error::io("stage write", e, &path))?;
        self.staged.lock().unwrap().insert(
            (root, staged.operation(), staged.resource_key().to_owned()),
            path,
        );
        Ok(())
    }
    pub fn clear_fault(&self) {
        *self.fault.lock().unwrap() = None;
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
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io("create_dir_all", e, parent.to_path_buf()))?;
        }
        std::fs::rename(&staged_path, &dest).map_err(|e| Error::io("rename", e, dest.clone()))?;
        self.staged.lock().unwrap().remove(&key);
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
