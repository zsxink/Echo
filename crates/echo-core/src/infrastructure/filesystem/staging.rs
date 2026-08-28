//! The controlled staging directory: random name, exclusive-create, ownership
//! marker (task 4.2 / design §8).
//!
//! Echo writes only inside a staging directory it created itself:
//!
//! - The name is `.echo-staging-<128-bit random hex>` and the directory is
//!   created with `create_dir` (exclusive at the filesystem level).
//! - Immediately after creation Echo writes `.echo-ownership-marker`, whose
//!   body carries the application magic, the owning `LibraryRootId` and a
//!   format version, and then reads it back to prove the write landed.
//! - If the chosen name already exists the directory is verified: only a
//!   directory with a fully matching marker (and no symlink) may be reused.
//!   Anything else — a user directory that happens to share the name, a
//!   forged/corrupt marker, a symlink or reparse point — makes Echo pick
//!   another random name. When no safe name can be established Echo disables
//!   its write capability for the root and **never** takes over, ignores or
//!   cleans up the foreign directory.

use std::path::{Path, PathBuf};

use crate::domain::ids::LibraryRootId;
use crate::error::{Error, PermKind};

use super::registry::RootRegistry;

/// Prefix every Echo staging directory carries.
pub const STAGING_DIR_PREFIX: &str = ".echo-staging-";
/// The marker file inside a staging directory.
pub const MARKER_FILE_NAME: &str = ".echo-ownership-marker";
/// Application magic: the first field of every marker body.
pub const MARKER_MAGIC: &str = "Echo controlled staging directory";
/// Marker format version.
pub const MARKER_FORMAT_VERSION: u32 = 1;
/// How many random names Echo tries before giving up (writes disabled).
const MAX_NAME_ATTEMPTS: usize = 8;

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

/// What the walker should do with a `.echo-staging-*` directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagingDecision {
    /// Marker matches: Echo's private directory — skip it entirely.
    Skip,
    /// Not owned by Echo: a user directory with a coincidental name. Scan it
    /// like any other user content; never ignore, take over or clean it.
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
    /// [`Error::Permission`] when no safe staging directory can be created —
    /// the caller must treat the root as not write-capable. Foreign
    /// directories are never touched, so repeated attempts with fresh random
    /// names are the only remedy.
    pub fn ensure_dir(&self, root: LibraryRootId) -> Result<PathBuf, Error> {
        if let Some((path, StagingCheck::Owned)) = self.existing_dir(root) {
            return Ok(path);
        }
        let base = self.registry.path_of(root)?;
        std::fs::create_dir_all(&base)
            .map_err(|source| Error::io("prepare root", source, &base))?;
        for _ in 0..MAX_NAME_ATTEMPTS {
            let candidate = base.join(format!("{STAGING_DIR_PREFIX}{}", random_128_bit_hex()));
            match std::fs::create_dir(&candidate) {
                Ok(()) => {
                    match write_marker(&candidate, root) {
                        Ok(()) => return Ok(candidate),
                        Err(error) => {
                            // The directory was created by us in this very
                            // attempt and owns nothing yet: remove the shell
                            // (best effort) and try a new name.
                            let _ = std::fs::remove_dir(&candidate);
                            tracing::debug!(root = %root, error = %error, "staging marker write failed; retrying with a new name");
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Someone owns this name. If it is genuinely our own
                    // directory from an earlier run, reuse it; otherwise pick
                    // another random name — never inspect, clean or take over.
                    if self.check(&candidate, root) == StagingCheck::Owned {
                        return Ok(candidate);
                    }
                }
                Err(error) => return Err(Error::io("create staging directory", error, candidate)),
            }
        }
        Err(Error::permission(
            "create staging directory",
            PermKind::NotOwner,
        ))
    }

    /// The current staging directory of `root`: the first `.echo-staging-*`
    /// directory whose marker fully matches. Foreign same-prefix directories
    /// are skipped, never adopted — an Echo-owned directory is still found
    /// when a user happens to have created one with the prefix earlier.
    fn existing_dir(&self, root: LibraryRootId) -> Option<(PathBuf, StagingCheck)> {
        let base = self.registry.path_of(root).ok()?;
        for entry in std::fs::read_dir(&base).ok()?.flatten() {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_DIR_PREFIX)
            {
                continue;
            }
            let path = entry.path();
            if self.check(&path, root) == StagingCheck::Owned {
                return Some((path, StagingCheck::Owned));
            }
        }
        None
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

/// A 128-bit random hex string for the directory name. Two v4 UUIDs (each 122
/// random bits) are folded through BLAKE3, whose 128-bit output prefix is
/// uniformly random — cheap, dependency-free and ample for name uniqueness.
fn random_128_bit_hex() -> String {
    let a = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    let b = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    let mut input = [0u8; 32];
    input[..16].copy_from_slice(&a);
    input[16..].copy_from_slice(&b);
    let hash = blake3::hash(&input);
    let bytes: [u8; 16] = hash.as_bytes()[..16].try_into().expect("16 of 32 bytes");
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
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

    #[test]
    fn staging_dir_is_created_with_matching_marker() {
        let (_dir, _registry, root, manager) = setup();
        let staging = manager.ensure_dir(root).unwrap();
        assert!(staging
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(STAGING_DIR_PREFIX));
        assert_eq!(manager.check(&staging, root), StagingCheck::Owned);
        assert!(manager.write_capable(root).unwrap());
        // Idempotent: the second call reuses the same directory.
        assert_eq!(manager.ensure_dir(root).unwrap(), staging);
    }

    #[test]
    fn forged_or_corrupt_marker_is_never_ours() {
        let (_dir, _registry, root, manager) = setup();
        let base = manager.registry.path_of(root).unwrap();
        let foreign = base.join(format!("{STAGING_DIR_PREFIX}forbidden"));
        std::fs::create_dir_all(&foreign).unwrap();

        // A user directory with the staging name but no marker.
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);
        assert_eq!(
            manager.walker_decision(&foreign, root),
            StagingDecision::Scan
        );

        // A forged marker with the wrong root id.
        std::fs::write(
            foreign.join(MARKER_FILE_NAME),
            StagingMarker::new(LibraryRootId::new()).body(),
        )
        .unwrap();
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);
        assert_eq!(
            manager.walker_decision(&foreign, root),
            StagingDecision::Scan
        );

        // A corrupt marker body.
        std::fs::write(foreign.join(MARKER_FILE_NAME), "not a marker").unwrap();
        assert_eq!(manager.check(&foreign, root), StagingCheck::NotOurs);

        // The manager neither took over nor cleaned the foreign directory.
        assert!(foreign.is_dir());
        ensure_dir_picks_a_safe_name(&manager, root, &foreign);
    }

    fn ensure_dir_picks_a_safe_name(manager: &StagingManager, root: LibraryRootId, foreign: &Path) {
        let staging = manager.ensure_dir(root).unwrap();
        assert_ne!(staging, foreign);
        assert_eq!(manager.check(&staging, root), StagingCheck::Owned);
        assert!(foreign.is_dir(), "foreign directory left untouched");
    }

    #[test]
    fn symlinked_staging_directory_is_never_followed() {
        let (dir, _registry, root, manager) = setup();
        let base = dir.path();
        let outside = tempfile::tempdir().unwrap();
        let link = base.join(format!("{STAGING_DIR_PREFIX}linked"));
        create_symlink(outside.path(), &link);
        assert_eq!(manager.check(&link, root), StagingCheck::NotOurs);
        assert_eq!(manager.walker_decision(&link, root), StagingDecision::Scan);
        let staging = manager.ensure_dir(root).unwrap();
        assert_ne!(staging, link, "symlink name must not be adopted");
    }

    /// Platform-neutral symlink creation for tests: `ln -s` on Unix-like
    /// systems, `mklink` through cmd on Windows. Not a `cfg` branch — the
    /// core forbids platform-conditional business code; this is test tooling
    /// choosing a helper program at runtime.
    fn create_symlink(target: &Path, link: &Path) {
        let status = if std::env::consts::OS == "windows" {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/D"])
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
    fn marker_round_trips_and_rejects_garbage() {
        let marker = StagingMarker::new(LibraryRootId::new());
        assert_eq!(StagingMarker::parse(&marker.body()), Some(marker));
        assert!(StagingMarker::parse("garbage").is_none());
        assert!(StagingMarker::parse("Echo controlled staging directory\nnope\n1\n").is_none());
    }
}
