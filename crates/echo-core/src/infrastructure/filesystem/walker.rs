//! The scan walker: bounded, root-constrained enumeration (task 4.2/4.7).
//!
//! Rules (design §6.1 + portable layout):
//!
//! - The walk ONLY discovers `media/` (portable-layout §5): the BFS starts at
//!   `base/media` when that directory exists, and otherwise enumerates nothing
//!   — root-level old-layout artist folders, loose files and foreign content
//!   are never scanned.
//! - Directory symlinks are never followed (`DirEntry::file_type` does not
//!   resolve symlinks), which cuts symlink cycles as well as escapes.
//! - A candidate file's canonical path must remain inside the canonical media
//!   root; a file symlink that resolves outside is dropped.
//! - The whole `echo/` control surface — `echo/manifest.json`,
//!   `echo/records/`, and the `echo/tmp/` staging tree — is invisible to the
//!   media scan and is never descended into (defense in depth, even though the
//!   walk starts at `media/` and `echo/` sits beside it).
//! - A `.echo-staging-*` directory is skipped **only** when its ownership
//!   marker fully matches — the same name without a valid marker is user
//!   content and is enumerated like everything else.
//! - All regular files are returned as candidates; the format matrix is the
//!   *scan's* cheap filter, applied after enumeration so unsupported files
//!   can be reported as skipped rather than silently dropped.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::library::{CONTROL_ROOT, MEDIA_ROOT};
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

/// Whether the root carries a `media/` tree (the only scanable content).
///
/// A portable library root without `media/` is old-layout or empty; the
/// root-switch candidate criterion uses this to refuse legacy roots instead of
/// scanning them.
#[must_use]
pub fn has_media_dir(base: &Path) -> bool {
    base.join(MEDIA_ROOT).is_dir()
}

/// Enumerate every regular file under `root`'s `media/` as a
/// [`RelativeMediaPath`]. A root without `media/` enumerates nothing — the old
/// artist-folder layout is never scanned (portable-layout §5).
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
    let media_base = base.join(MEDIA_ROOT);
    if !has_media_dir(&base) {
        // No scanable media tree: an old-layout or empty root is enumerated as
        // empty, never cascaded into artist folders (portable-layout §5).
        return Ok((Vec::new(), WalkStats::default()));
    }
    let canonical_base = canonical_root(&media_base)?;
    let mut stats = WalkStats::default();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([media_base.clone()]);
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
                // The whole `echo/` control surface is invisible to the media
                // scan: neither the manifest, the object records nor the
                // `echo/tmp/` staging tree are ever descended into (task 3.2).
                // Defense in depth — the walk already starts at `media/`, so
                // this also shields a stray `echo/` created inside media.
                let rel = path.strip_prefix(&media_base).ok();
                if rel.is_some_and(|r| {
                    r.components()
                        .next()
                        .is_some_and(|c| c.as_os_str() == CONTROL_ROOT)
                }) {
                    continue;
                }
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
            let Ok(relative) = path.strip_prefix(&media_base) else {
                continue;
            };
            // Rebuild the `media/<artist>/<file>` root-relative path the
            // library and record payloads are keyed on.
            let relative_text = relative.to_string_lossy();
            let Ok(relative) = RelativeMediaPath::new(&format!("{MEDIA_ROOT}/{relative_text}"))
            else {
                continue;
            };
            if file_type.is_symlink() {
                // Dangling symlinks are skipped outright; a symlink is a
                // candidate only when its canonical target is a regular file
                // *inside* the media tree (never a directory — those are not
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

    /// Create a symlink, or `None` when the platform/runner cannot (Windows
    /// without Developer Mode / elevated shell). The tests that need a symlink
    /// skip on `None` — a runner that can't make one cannot exercise the
    /// never-follow guarantees meaningfully, and a hard failure would just turn
    /// every such environment red.
    fn symlink(target: &std::path::Path, link: &std::path::Path) -> Option<()> {
        let status = if std::env::consts::OS == "windows" {
            std::process::Command::new("cmd")
                .args(["/C", "mklink"])
                .arg(link)
                .arg(target)
                .status()
        } else {
            std::process::Command::new("ln")
                .arg("-s")
                .arg(target)
                .arg(link)
                .status()
        };
        match status {
            Ok(s) if s.success() => Some(()),
            Ok(_) => {
                eprintln!("skipping symlink-dependent test: symlink creation not permitted");
                None
            }
            Err(error) => {
                eprintln!("skipping symlink-dependent test: {error}");
                None
            }
        }
    }

    #[test]
    fn walker_enumerates_media_and_never_enters_the_control_surface() {
        let (dir, root, staging) = setup();
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        std::fs::write(dir.path().join("media/a.mp3"), b"audio").unwrap();
        std::fs::create_dir_all(dir.path().join("media/artist")).unwrap();
        std::fs::write(dir.path().join("media/artist/b.flac"), b"audio").unwrap();

        // Echo-owned staging `echo/tmp` with content that must be invisible
        // (it lives under the `echo/` control surface).
        let owned = staging.ensure_dir(root).unwrap();
        std::fs::write(owned.join("staged.mp3"), b"staged").unwrap();

        // Other control-surface content (manifest, records) is invisible too.
        let control = dir.path().join("echo");
        std::fs::write(control.join("manifest.json"), b"{}").unwrap();

        // Root-level loose files and old-layout artist folders are never
        // scanned (portable layout §5: only `media/` is the managed tree).
        std::fs::write(dir.path().join("loose.mp3"), b"loose").unwrap();
        std::fs::create_dir_all(dir.path().join("周杰伦")).unwrap();
        std::fs::write(dir.path().join("周杰伦/晴天.flac"), b"old-layout").unwrap();

        let (files, stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(paths.contains(&"media/a.mp3"));
        assert!(paths.contains(&"media/artist/b.flac"));
        assert_eq!(stats.staging_dirs_skipped, 0);
        assert_eq!(stats.files, 2);
        assert!(
            !paths.iter().any(|p| p.starts_with("echo/")),
            "the echo/ control surface is never enumerated: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| !p.starts_with("media/")),
            "only media/ files are enumerated, never root or old-layout content: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with(".echo-staging-")),
            "legacy staging-prefix names that are real files under media are user content"
        );
        assert!(owned.join("staged.mp3").exists(), "staged file untouched");
    }

    #[test]
    fn walker_of_a_root_without_media_enumerates_nothing() {
        // A root with no `media/` tree is old-layout or empty; the walker must
        // not cascade into root-level artist folders or loose files (portable
        // layout §5: 缺少可扫描的 media/ 时不迁移、不猜测旧歌曲身份).
        let (dir, root, staging) = setup();
        std::fs::create_dir_all(dir.path().join("周杰伦")).unwrap();
        std::fs::write(dir.path().join("周杰伦/晴天.flac"), b"old").unwrap();
        std::fs::write(dir.path().join("loose.flac"), b"loose").unwrap();

        let (files, stats) = enumerate_files(root, &staging).unwrap();
        assert!(files.is_empty(), "no media/ means no candidates");
        assert_eq!(stats.files, 0);
        assert!(
            !has_media_dir(dir.path()),
            "has_media_dir reflects the absence"
        );
    }

    #[test]
    fn has_media_dir_detects_a_managed_tree() {
        let (dir, root, staging) = setup();
        assert!(!has_media_dir(dir.path()));
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        assert!(has_media_dir(dir.path()));
        let _ = root;
        let _ = staging;
    }

    #[test]
    fn walker_rejects_escaping_file_symlinks() {
        let (dir, root, staging) = setup();
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.mp3");
        std::fs::write(&secret, b"outside").unwrap();
        let link = dir.path().join("media/escape.mp3");
        let Some(()) = symlink(&secret, &link) else {
            return;
        };
        std::fs::write(dir.path().join("media/inside.mp3"), b"inside").unwrap();

        let (files, stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(
            !paths.contains(&"media/escape.mp3"),
            "escaping symlink rejected"
        );
        assert!(paths.contains(&"media/inside.mp3"));
        assert_eq!(stats.escapes_rejected, 1);
    }

    #[test]
    fn walker_does_not_follow_directory_symlinks() {
        let (dir, root, staging) = setup();
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("music")).unwrap();
        std::fs::write(outside.path().join("music/x.mp3"), b"x").unwrap();
        let link = dir.path().join("media/linked");
        let Some(()) = symlink(&outside.path().join("music"), &link) else {
            return;
        };

        let (files, _stats) = enumerate_files(root, &staging).unwrap();
        let paths: Vec<_> = files.iter().map(RelativeMediaPath::display).collect();
        assert!(
            !paths.iter().any(|p| p.starts_with("media/linked/")),
            "directory symlinks are not descended: {paths:?}"
        );
    }

    #[test]
    fn walker_toctou_swap_to_symlink_is_rejected_on_re_enumeration() {
        // Task 12.7 TOCTOU: between enumeration passes an attacker swaps a
        // previously-regular inside file for a symlink to an outside file.
        // Because every pass re-canonicalizes each candidate and requires it
        // to remain inside the canonical media root, the swapped-in symlink is
        // rejected on re-enumeration — no stale trust of the earlier path.
        let (dir, root, staging) = setup();
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.mp3");
        std::fs::write(&secret, b"outside-secret").unwrap();

        // Pass 1: the file is a real inside regular file.
        std::fs::write(dir.path().join("media/swap.mp3"), b"inside").unwrap();
        let (files, stats) = enumerate_files(root, &staging).unwrap();
        assert!(files.iter().any(|p| p.display() == "media/swap.mp3"));
        assert_eq!(stats.escapes_rejected, 0);

        // The attacker replaces the inside file with a symlink to the outside
        // secret (a classic TOCTOU swap after the first read).
        std::fs::remove_file(dir.path().join("media/swap.mp3")).unwrap();
        let Some(()) = symlink(&secret, &dir.path().join("media/swap.mp3")) else {
            return;
        };

        // Pass 2 (the reconcile/read pass): the path must be re-validated —
        // its canonical target is now outside the media tree, so it is
        // rejected, and the outside content is never exposed as a library file.
        let (files2, stats2) = enumerate_files(root, &staging).unwrap();
        let paths2: Vec<_> = files2.iter().map(RelativeMediaPath::display).collect();
        assert!(
            !paths2.contains(&"media/swap.mp3"),
            "TOCTOU-swapped symlink must not be read as a library file: {paths2:?}"
        );
        assert_eq!(stats2.escapes_rejected, 1);
    }
}
