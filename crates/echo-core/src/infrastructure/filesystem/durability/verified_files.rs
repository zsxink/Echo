//! Verify a checkpoint's materialized files without resolving the same parent
//! directories thousands of times. Handles and evidence are local to one read;
//! no file contents or filesystem metadata are trusted between operations.

#[cfg(not(unix))]
use super::portable;
use super::{invalid, LIMIT};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use crate::infrastructure::filesystem::RootRegistry;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn verify(
    registry: &RootRegistry,
    root: LibraryRootId,
    files: &BTreeMap<String, String>,
    newer: &BTreeSet<String>,
) -> Result<(), Error> {
    read_verified(registry, root, files, newer, LIMIT, |_, _| Ok(()))
}

#[cfg(not(unix))]
pub(super) fn read_verified(
    registry: &RootRegistry,
    root: LibraryRootId,
    files: &BTreeMap<String, String>,
    newer: &BTreeSet<String>,
    limit: usize,
    mut visit: impl FnMut(&str, &str) -> Result<(), Error>,
) -> Result<(), Error> {
    for (path, digest) in files {
        if newer.contains(path) {
            continue;
        }
        let raw = portable::read(registry, root, path, limit)?
            .ok_or_else(|| invalid("checkpoint file missing"))?;
        if blake3::hash(raw.as_bytes()).to_hex().as_str() != digest {
            return Err(invalid("checkpoint file digest mismatch"));
        }
        visit(path, &raw)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn read_verified(
    registry: &RootRegistry,
    root: LibraryRootId,
    files: &BTreeMap<String, String>,
    newer: &BTreeSet<String>,
    limit: usize,
    visit: impl FnMut(&str, &str) -> Result<(), Error>,
) -> Result<(), Error> {
    verify_with_audit_hook(registry, root, files, newer, limit, visit, || {})
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)] // Keep opening and auditing the same namespace proof together.
fn verify_with_audit_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    files: &BTreeMap<String, String>,
    newer: &BTreeSet<String>,
    limit: usize,
    mut visit: impl FnMut(&str, &str) -> Result<(), Error>,
    before_audit: impl FnOnce(),
) -> Result<(), Error> {
    use crate::domain::ids::RelativeMediaPath;
    use rustix::fs::{open, openat, Mode, OFlags};
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    let base = registry.path_of(root)?;
    let root_metadata = std::fs::symlink_metadata(&base)
        .map_err(|error| Error::io("inspect library root", error, &base))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(invalid("library root is not a directory"));
    }
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root_handle = std::fs::File::from(
        open(&base, directory_flags, Mode::empty())
            .map_err(|error| Error::io("open library root", error.into(), &base))?,
    );
    let opened_root = root_handle
        .metadata()
        .map_err(|error| Error::io("inspect library root", error, &base))?;
    if (opened_root.dev(), opened_root.ino()) != (root_metadata.dev(), root_metadata.ino()) {
        return Err(invalid("library root changed during verification"));
    }
    let mut directory_evidence =
        BTreeMap::from([(base.clone(), (opened_root.dev(), opened_root.ino()))]);
    let mut stack: Vec<(String, std::fs::File)> = Vec::new();
    for (relative, digest) in files {
        if newer.contains(relative) {
            continue;
        }
        let checked = RelativeMediaPath::new(relative)?;
        let components: Vec<_> = checked.normalized().split('/').collect();
        let (filename, parents) = components
            .split_last()
            .ok_or_else(|| invalid("empty checkpoint path"))?;
        let common = stack
            .iter()
            .zip(parents)
            .take_while(|((name, _), parent)| name == **parent)
            .count();
        stack.truncate(common);
        let mut directory_path: PathBuf = base.clone();
        for parent in &parents[..common] {
            directory_path.push(parent);
        }
        for parent in &parents[common..] {
            directory_path.push(parent);
            let parent_handle = stack.last().map_or(&root_handle, |(_, file)| file);
            let handle = std::fs::File::from(
                openat(parent_handle, *parent, directory_flags, Mode::empty()).map_err(
                    |error| Error::io("open checkpoint directory", error.into(), &directory_path),
                )?,
            );
            let metadata = handle.metadata().map_err(|error| {
                Error::io("inspect checkpoint directory", error, &directory_path)
            })?;
            let identity = (metadata.dev(), metadata.ino());
            if directory_evidence
                .insert(directory_path.clone(), identity)
                .is_some_and(|before| before != identity)
            {
                return Err(invalid("checkpoint directory changed during verification"));
            }
            stack.push(((*parent).to_owned(), handle));
        }
        let parent_handle = stack.last().map_or(&root_handle, |(_, file)| file);
        let file_path = base.join(checked.normalized());
        let file = std::fs::File::from(
            openat(
                parent_handle,
                *filename,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|error| Error::io("open checkpoint file", error.into(), &file_path))?,
        );
        if !file
            .metadata()
            .map_err(|error| Error::io("inspect checkpoint file", error, &file_path))?
            .is_file()
        {
            return Err(invalid("checkpoint file is not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| Error::io("read checkpoint file", error, &file_path))?;
        if bytes.len() > limit {
            return Err(invalid("invalid checkpoint file contents"));
        }
        let raw =
            std::str::from_utf8(&bytes).map_err(|_| invalid("checkpoint file is not UTF-8"))?;
        if blake3::hash(&bytes).to_hex().as_str() != digest {
            return Err(invalid("checkpoint file digest mismatch"));
        }
        visit(relative, raw)?;
    }
    // A retained directory handle remains usable after rename or replacement.
    // Check every traversed namespace edge again before trusting the proof.
    before_audit();
    if registry.path_of(root)? != base {
        return Err(invalid("library root binding changed during verification"));
    }
    for (path, expected) in directory_evidence {
        let actual = std::fs::symlink_metadata(&path)
            .map_err(|error| Error::io("recheck checkpoint directory", error, &path))?;
        if actual.file_type().is_symlink()
            || !actual.is_dir()
            || (actual.dev(), actual.ino()) != expected
        {
            return Err(invalid("checkpoint directory changed during verification"));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn verified_checkpoint_files_reject_tampering_and_symlinked_directories() {
        let directory = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, directory.path());
        let relative = "echo/records/song/aa/example.json";
        let path = directory.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"original").unwrap();
        let files = BTreeMap::from([(
            relative.to_owned(),
            blake3::hash(b"original").to_hex().to_string(),
        )]);
        verify(&registry, root, &files, &BTreeSet::new()).unwrap();
        std::fs::write(&path, b"tampered").unwrap();
        assert!(verify(&registry, root, &files, &BTreeSet::new()).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let outside = directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("example.json"), b"original").unwrap();
        std::os::unix::fs::symlink(&outside, path.parent().unwrap()).unwrap();
        assert!(verify(&registry, root, &files, &BTreeSet::new()).is_err());
    }

    #[test]
    fn verified_checkpoint_files_reject_directory_replacement_during_read() {
        let directory = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, directory.path());
        let relative = "echo/records/song/aa/example.json";
        let path = directory.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"original").unwrap();
        let files = BTreeMap::from([(
            relative.to_owned(),
            blake3::hash(b"original").to_hex().to_string(),
        )]);
        let parent = path.parent().unwrap().to_path_buf();
        let moved = parent.with_file_name("moved");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let result = verify_with_audit_hook(
            &registry,
            root,
            &files,
            &BTreeSet::new(),
            LIMIT,
            |_, _| Ok(()),
            || {
                std::fs::rename(&parent, &moved).unwrap();
                std::os::unix::fs::symlink(&outside, &parent).unwrap();
            },
        );
        assert!(result.is_err());
    }

    #[test]
    #[ignore = "repeatable local filesystem performance measurement"]
    fn compare_checkpoint_verification_paths() {
        let directory = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, directory.path());
        let mut files = BTreeMap::new();
        for index in 0..1000 {
            let relative = format!("echo/records/song/{:02x}/{index:032x}.json", index % 256);
            let path = directory.path().join(&relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = format!("{{\"id\":{index}}}");
            std::fs::write(path, &bytes).unwrap();
            files.insert(
                relative,
                blake3::hash(bytes.as_bytes()).to_hex().to_string(),
            );
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            for (path, digest) in &files {
                let raw = super::super::portable::read(&registry, root, path, LIMIT)
                    .unwrap()
                    .unwrap();
                assert_eq!(blake3::hash(raw.as_bytes()).to_hex().as_str(), digest);
            }
            before.push(start.elapsed());
            let start = std::time::Instant::now();
            verify(&registry, root, &files, &BTreeSet::new()).unwrap();
            after.push(start.elapsed());
        }
        before.sort();
        after.sort();
        eprintln!(
            "checkpoint files=1000 p95 before={:.2}ms after={:.2}ms",
            before[18].as_secs_f64() * 1000.0,
            after[18].as_secs_f64() * 1000.0
        );
    }
}
