//! The scan walker: bounded, root-constrained enumeration (task 4.2/4.7).
//!
//! Rules (design §6.1):
//!
//! - Directory symlinks are never followed (`DirEntry::file_type` does not
//!   resolve symlinks), which cuts symlink cycles as well as escapes.
//! - A candidate file's canonical path must remain inside the canonical root;
//!   a file symlink that resolves outside the root is dropped.
//! - A `.echo-staging-*` directory is skipped **only** when its ownership
//!   marker fully matches — the same name without a valid marker is user
//!   content and is enumerated like everything else.
//! - All regular files are returned as candidates; the format matrix is the
//!   *scan's* cheap filter, applied after enumeration so unsupported files
//!   can be reported as skipped rather than silently dropped.

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::error::Error;

use super::staging::{StagingDecision, StagingManager, STAGING_DIR_PREFIX};

/// Statistics of one walk — surfaced for diagnostics, not identity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WalkStats {
    /// Regular files enumerated (candidates).
    pub files: usize,
    /// Directories skipped because their staging marker matched.
    pub staging_dirs_skipped: usize,
    /// Files dropped because their canonical path left the root.
    pub escapes_rejected: usize,
}

/// Enumerate every regular file under `root` as a [`RelativeMediaPath`].
///
/// # Errors
///
/// Propagates an unreadable root (unavailable); per-entry failures inside are
/// skipped individually — a single unreadable subdirectory must not abort the
/// enumeration of the rest.
pub fn enumerate_files(
    root: LibraryRootId,
    staging: &StagingManager,
) -> Result<(Vec<RelativeMediaPath>, WalkStats), Error> {
    let base = staging.registry().path_of(root)?;
    let canonical_base = canonical_root(&base)?;
    let mut stats = WalkStats::default();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([base.clone()]);
    while let Some(dir) = queue.pop_front() {
        // Any read_dir failure aborts the enumeration: an interrupted walk
        // must never masquerade as a complete one (design §5 — 枚举中断不得
        // 被当作成功; the root-switch candidate criterion depends on it).
        let entries = std::fs::read_dir(&dir).map_err(|source| {
            Error::unavailable_with_source("library enumeration", "directory unreadable", source)
        })?;
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(STAGING_DIR_PREFIX)
                    && staging.walker_decision(&path, root) == StagingDecision::Skip
                {
                    stats.staging_dirs_skipped += 1;
                    continue;
                }
                queue.push_back(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(&base) else {
                continue;
            };
            let Ok(relative) = RelativeMediaPath::new(&relative.to_string_lossy()) else {
                continue;
            };
            if file_type.is_symlink() {
                // Dangling symlinks are skipped outright; a symlink is a
                // candidate only when its canonical target is a regular file
                // *inside* the root (never a directory — those are not
                // descended and never become bogus candidates).
                let Ok(canonical) = path.canonicalize() else {
                    continue;
                };
                if !canonical.starts_with(&canonical_base) {
                    stats.escapes_rejected += 1;
                    continue;
                }
                if !canonical.is_file() {
                    continue;
                }
            } else if !file_type.is_file() {
                // Special files (FIFOs, sockets, devices) are never candidates.
                continue;
            }
            stats.files += 1;
            out.push(relative);
        }
    }
    Ok((out, stats))
}

fn canonical_root(base: &std::path::Path) -> Result<PathBuf, Error> {
    base.canonicalize()
        .map_err(|source| Error::unavailable_with_source("library root", "root unreadable", source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::filesystem::registry::RootRegistry;

    fn setup() -> (tempfile::TempDir, LibraryRootId, StagingManager) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let staging = StagingManager::new(registry);
        (dir, root, staging)
    }

    fn symlink(target: &std::path::Path, link: &std::path::Path) {
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
    fn walker_enumerates_files_and_skips_only_owned_staging_dirs() {
        let (dir, root, staging) = setup();
        std::fs::write(dir.path().join("a.mp3"), b"audio").unwrap();
        std::fs::create_dir_all(dir.path().join("artist")).unwrap();
        std::fs::write(dir.path().join("artist/b.flac"), b"audio").unwrap();

        // Echo-owned staging dir with content that must be invisible.
        let owned = staging.ensure_dir(root).unwrap();
        std::fs::write(owned.join("staged.mp3"), b"staged").unwrap();

        // Foreign same-prefix directory with a file that must be visible.
        let foreign = dir.path().join(".echo-staging-user-content");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("user.mp3"), b"user").unwrap();

        let (files, stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(paths.contains(&"a.mp3"));
        assert!(paths.contains(&"artist/b.flac"));
        assert!(paths.contains(&".echo-staging-user-content/user.mp3"));
        assert_eq!(stats.staging_dirs_skipped, 1);
        assert_eq!(stats.files, 3);
        assert!(owned.join("staged.mp3").exists(), "staged file untouched");
    }

    #[test]
    fn walker_rejects_escaping_file_symlinks() {
        let (dir, root, staging) = setup();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.mp3");
        std::fs::write(&secret, b"outside").unwrap();
        let link = dir.path().join("escape.mp3");
        symlink(&secret, &link);
        std::fs::write(dir.path().join("inside.mp3"), b"inside").unwrap();

        let (files, stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(!paths.contains(&"escape.mp3"), "escaping symlink rejected");
        assert!(paths.contains(&"inside.mp3"));
        assert_eq!(stats.escapes_rejected, 1);
    }

    #[test]
    fn walker_does_not_follow_directory_symlinks() {
        let (dir, root, staging) = setup();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("music")).unwrap();
        std::fs::write(outside.path().join("music/x.mp3"), b"x").unwrap();
        let link = dir.path().join("linked");
        symlink(&outside.path().join("music"), &link);

        let (files, _stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(
            !paths.iter().any(|p| p.starts_with("linked/")),
            "directory symlinks are not descended: {paths:?}"
        );
    }
}
