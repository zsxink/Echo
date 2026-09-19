//! The controlled staging directory: the portable `echo/tmp/` tree under the
//! library root, exclusive-create, ownership marker (task 3.2 / design §8).
//!
//! Echo writes only inside a staging directory it created itself:
//!
//! - The directory is the fixed portable `echo/tmp/` under the library root
//!   (portable-library-layout spec); `create_dir` (exclusive at the
//!   filesystem level) establishes it on first use.
//! - Immediately after creation Echo writes `.echo-ownership-marker`, whose
//!   body carries the application magic, the owning `LibraryRootId` and a
//!   format version, and then reads it back to prove the write landed.
//! - If `echo/tmp` already exists the directory is verified: only a directory
//!   with a fully matching marker (and no symlink) may be reused. Anything
//!   else — a user-created `echo/tmp` without our marker, a forged/corrupt
//!   marker, a symlink or reparse point — makes the root *not write-capable*:
//!   Echo **never** takes over, ignores into, or cleans up the foreign
//!   directory (the control surface is reserved, not assumed).
//!
//! The marker file keeps its historical name/body so an `echo/tmp` created by
//! an earlier run is verified the same way; nothing about the ownership proof
//! changes, only the directory it lives in.

use std::path::{Path, PathBuf};

use crate::domain::ids::LibraryRootId;
use crate::domain::library::STAGING_ROOT;
use crate::error::{Error, PermKind};

use super::registry::RootRegistry;

/// The marker file inside a staging directory.
pub const MARKER_FILE_NAME: &str = ".echo-ownership-marker";
/// Application magic: the first field of every marker body.
pub const MARKER_MAGIC: &str = "Echo controlled staging directory";
/// Marker format version.
pub const MARKER_FORMAT_VERSION: u32 = 1;
/// Kept for historical references to the random-name scheme that the portable
/// layout replaced (`echo/tmp` is now the single fixed staging root).
pub const STAGING_DIR_PREFIX: &str = ".echo-staging-";

/// Why a directory does or does not belong to Echo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StagingCheck {
    /// Echo owns this directory: marker fully matches the root.
    Owned,
    /// Not Echo's (foreign content, corrupt/forged marker, symlink).
    NotOurs,
    /// The directory does not exist at all.
    Missing,
}

/// What the walker should do with the portable `echo/` control surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagingDecision {
    /// Marker matches: Echo's private staging tree — skip it entirely.
    Skip,
    /// Not owned by Echo: a user directory in the reserved control path. The
    /// media scan never enters `echo/` regardless, so this arm exists for
    /// callers that ask explicitly; Echo never ignores it into a scan.
    Scan,
}

/// Creates, verifies and locates staging directories for library roots.
#[derive(Clone, Debug)]
pub struct StagingManager {
    registry: RootRegistry,
}

impl StagingManager {
    #[must_use]
    pub const fn new(registry: RootRegistry) -> Self {
        Self { registry }
    }

    /// The registry this manager resolves roots against.
    #[must_use]
    pub const fn registry(&self) -> &RootRegistry {
        &self.registry
    }

    /// The staging directory of `root`, establishing it if needed.
    ///
    /// # Errors
    ///
    /// [`Error::Permission`] when the portable `echo/tmp` cannot be created
    /// or is not an Echo-owned directory (a user-created or forged/symlinked
    /// `echo/tmp`). The caller must then treat the root as not write-capable;
    /// Echo never stages into, takes over, or cleans up a foreign control
    /// surface.
    pub fn ensure_dir(&self, root: LibraryRootId) -> Result<PathBuf, Error> {
        if let Some((path, StagingCheck::Owned)) = self.existing_dir(root) {
            return Ok(path);
        }
        let base = self.registry.path_of(root)?;
        std::fs::create_dir_all(&base)
            .map_err(|source| Error::io("prepare root", source, &base))?;
        // Ensure the parent `echo/` exists before trying exclusive-create on
        // `echo/tmp`; `create_dir_all` is idempotent when `echo/` is already
        // present (our own marker-owned dir or an already-existing parent).
        let parent = base.join(crate::domain::library::CONTROL_ROOT);
        std::fs::create_dir_all(&parent)
            .map_err(|source| Error::io("prepare control surface", source, parent.clone()))?;
        let candidate = base.join(STAGING_ROOT);
        match std::fs::create_dir(&candidate) {
            Ok(()) => write_marker(&candidate, root).map(|()| candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Someone owns `echo/tmp`. It is only ours when the marker
                // fully matches; otherwise the root is not write-capable and
                // the directory is never inspected, cleaned or taken over.
                if self.check(&candidate, root) == StagingCheck::Owned {
                    Ok(candidate)
                } else {
                    Err(Error::permission(
                        "create staging directory",
                        PermKind::NotOwner,
                    ))
                }
            }
            Err(error) => Err(Error::io("create staging directory", error, candidate)),
        }
    }

    /// The current staging directory of `root`: the portable `echo/tmp`
    /// directory whose marker fully matches. A foreign/corrupt/symlinked
    /// `echo/tmp` is never adopted.
    fn existing_dir(&self, root: LibraryRootId) -> Option<(PathBuf, StagingCheck)> {
        let base = self.registry.path_of(root).ok()?;
        let path = base.join(STAGING_ROOT);
        if self.check(&path, root) == StagingCheck::Owned {
            Some((path, StagingCheck::Owned))
        } else {
            None
        }
    }

    /// Verify one directory against `root` (marker + symlink checks).
    #[must_use]
    pub fn check(&self, path: &Path, root: LibraryRootId) -> StagingCheck {
        if !path.exists() {
            return StagingCheck::Missing;
        }
        // A symlinked directory is never ours regardless of marker content:
        // following it would hand Echo's writes to whoever owns the target.
        if path.is_symlink() {
            return StagingCheck::NotOurs;
        }
        let is_owned = std::fs::read(path.join(MARKER_FILE_NAME))
            .ok()
            .and_then(|bytes| StagingMarker::parse(&String::from_utf8_lossy(&bytes)))
            .is_some_and(|marker| marker.root == root && marker.is_current_format());
        if is_owned {
            StagingCheck::Owned
        } else {
            StagingCheck::NotOurs
        }
    }

    /// The walker's skip decision for a `.echo-staging-*` directory.
    #[must_use]
    pub fn walker_decision(&self, path: &Path, root: LibraryRootId) -> StagingDecision {
        match self.check(path, root) {
            StagingCheck::Owned => StagingDecision::Skip,
            StagingCheck::NotOurs | StagingCheck::Missing => StagingDecision::Scan,
        }
    }

    /// Whether `root` currently allows writes: a staging directory Echo owns.
    ///
    /// # Errors
    ///
    /// Propagates the root-lookup failure when the root is not bound.
    pub fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        Ok(matches!(
            self.existing_dir(root),
            Some((_, StagingCheck::Owned))
        ))
    }

    /// The absolute path of `root`'s owned staging root (`echo/tmp`), when
    /// established. `None` when the root is not write-capable (no owned
    /// `echo/tmp`).
    #[must_use]
    pub fn staging_root(&self, root: LibraryRootId) -> Option<PathBuf> {
        self.existing_dir(root).map(|(path, _)| path)
    }

    /// Whether `path` resolves inside `root`'s owned `echo/tmp` staging tree
    /// (walking up from the file to the `echo/tmp` ancestor that carries a
    /// fully matching marker). A file/dir under a foreign, corrupt or symlinked
    /// `echo/tmp` is never recognized as Echo's own — the boundary is the
    /// marker, not the name.
    #[must_use]
    pub fn owned_staging_ancestor(&self, path: &Path, root: LibraryRootId) -> bool {
        let Some(owned) = self.staging_root(root) else {
            return false;
        };
        path.ancestors().any(|p| p == owned)
    }
}

/// The parsed ownership marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagingMarker {
    pub root: LibraryRootId,
    pub version: u32,
}

impl StagingMarker {
    #[must_use]
    pub const fn new(root: LibraryRootId) -> Self {
        Self {
            root,
            version: MARKER_FORMAT_VERSION,
        }
    }

    /// The marker body: `magic \n root-uuid \n version`.
    #[must_use]
    pub fn body(&self) -> String {
        format!("{MARKER_MAGIC}\n{}\n{}\n", self.root, self.version)
    }

    /// Parse a marker body; `None` when magic, UUID or version is malformed.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let magic = lines.next()?;
        if magic.trim() != MARKER_MAGIC {
            return None;
        }
        let root = lines.next()?.trim().parse().ok()?;
        let version = lines.next()?.trim().parse().ok()?;
        Some(Self { root, version })
    }

    const fn is_current_format(&self) -> bool {
        self.version == MARKER_FORMAT_VERSION
    }
}

fn write_marker(dir: &Path, root: LibraryRootId) -> Result<(), Error> {
    let marker_path = dir.join(MARKER_FILE_NAME);
    let marker = StagingMarker::new(root);
    std::fs::write(&marker_path, marker.body())
        .map_err(|source| Error::io("write staging marker", source, &marker_path))?;
    // Read-back: the ownership claim is only trusted once it round-trips.
    let written = std::fs::read(&marker_path)
        .map_err(|source| Error::io("verify staging marker", source, &marker_path))?;
    if String::from_utf8_lossy(&written) != marker.body() {
        return Err(Error::permission(
            "verify staging marker",
            PermKind::NotOwner,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (
        tempfile::TempDir,
        RootRegistry,
        LibraryRootId,
        StagingManager,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let manager = StagingManager::new(registry.clone());
        (dir, registry, root, manager)
    }

    fn staging_error_code(result: Result<PathBuf, Error>) -> Option<&'static str> {
        result.map_err(|error| error.code()).err()
    }

    #[test]
    fn staging_dir_is_emitted_as_echo_tmp_with_matching_marker() {
        let (dir, _registry, root, manager) = setup();
        let staging = manager.ensure_dir(root).unwrap();
        assert_eq!(
            staging,
            dir.path().join("echo/tmp"),
            "the portable layout staging root is exactly echo/tmp"
        );
        assert_eq!(manager.check(&staging, root), StagingCheck::Owned);
        assert!(manager.write_capable(root).unwrap());
        // Idempotent: the second call reuses the same directory.
        assert_eq!(manager.ensure_dir(root).unwrap(), staging);
    }

    #[test]
    fn foreign_or_corrupt_echo_tmp_never_becomes_ours() {
        let (_dir, _registry, root, manager) = setup();
        let base = manager.registry.path_of(root).unwrap();
        let foreign = base.join("echo/tmp");
        std::fs::create_dir_all(&foreign).unwrap();

        // A user directory occupying `echo/tmp` with no marker: not ours.
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);
        assert_eq!(
            manager.walker_decision(&foreign, root),
            StagingDecision::Scan
        );
        // `ensure_dir` cannot adopt it and the root becomes non-write-capable:
        // Echo never stages into the reserved control path.
        assert_eq!(
            staging_error_code(manager.ensure_dir(root)),
            Some("permission")
        );
        assert!(!manager.write_capable(root).unwrap());
        assert!(foreign.is_dir(), "the foreign echo/tmp is left untouched");

        // A forged marker with the wrong root id is still not ours.
        std::fs::write(
            foreign.join(MARKER_FILE_NAME),
            StagingMarker::new(LibraryRootId::new()).body(),
        )
        .unwrap();
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);
        assert_eq!(
            staging_error_code(manager.ensure_dir(root)),
            Some("permission")
        );

        // A corrupt marker body.
        std::fs::write(foreign.join(MARKER_FILE_NAME), "not a marker").unwrap();
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);
        assert!(
            manager.ensure_dir(root).is_err(),
            "a corrupt marker still refuses the root"
        );
        assert!(foreign.is_dir(), "foreign echo/tmp left untouched");
    }

    #[test]
    fn symlinked_staging_directory_is_never_followed() {
        let (dir, _registry, root, manager) = setup();
        let base = dir.path();
        let outside = tempfile::tempdir().unwrap();
        let link = base.join("echo/tmp");
        std::fs::create_dir_all(base.join("echo")).unwrap();
        let Some(()) = create_symlink(outside.path(), &link) else {
            return;
        };
        assert_eq!(manager.check(&link, root), StagingCheck::NotOurs);
        assert_eq!(manager.walker_decision(&link, root), StagingDecision::Scan);
        assert!(
            manager.ensure_dir(root).is_err(),
            "a symlinked echo/tmp must not be adopted"
        );
        assert_eq!(
            staging_error_code(manager.ensure_dir(root)),
            Some("permission")
        );
    }

    /// Platform-neutral symlink creation for tests: `ln -s` on Unix-like
    /// systems, `mklink` through cmd on Windows. Not a `cfg` branch — the
    /// core forbids platform-conditional business code; this is test tooling
    /// choosing a helper program at runtime. Returns `None` when the runner
    /// cannot create symlinks (e.g. Windows without Developer Mode), so the
    /// test skips instead of failing on an environment that cannot exercise
    /// the guarantee.
    fn create_symlink(target: &Path, link: &Path) -> Option<()> {
        let status = if std::env::consts::OS == "windows" {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/D"])
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
            _ => {
                eprintln!("skipping symlink-dependent test: symlink creation not permitted");
                None
            }
        }
    }

    #[test]
    fn marker_round_trips_and_rejects_garbage() {
        let marker = StagingMarker::new(LibraryRootId::new());
        assert_eq!(StagingMarker::parse(&marker.body()), Some(marker));
        assert!(StagingMarker::parse("garbage").is_none());
        assert!(StagingMarker::parse("Echo controlled staging directory\nnope\n1\n").is_none());
    }
}
