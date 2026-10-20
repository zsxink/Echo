//! Portable library layout domain types (task 1.1).
//!
//! Defines the portable, versioned library surface that makes media and logical
//! records restorable across devices without carrying `SQLite`, credentials or
//! absolute paths.  These types live in the Domain layer: they are pure value
//! objects and validators with no dependency on the file system or database.
//!
//! Layout (design §1, portable-library-layout spec):
//!
//! ```text
//! <library-root>/
//! ├── media/<artist>/<artist> - <title>.<ext>
//! └── echo/
//!     ├── manifest.json
//!     └── records/<kind>/<prefix>/<uuid>.json
//! ```
//!
//! [`ControlPath`] validates paths *within* the `echo/` control surface.
//! [`LibraryRelativePath`] validates safe paths within the library root,
//! including legacy paths outside the canonical `media/` tree. Neither type
//! carries an absolute path, `..`, or a path that would escape the root.
//!
//! [`PortableRecordDocument`] reads the historical flat [`PortableRecord`] and
//! the v2 [`PortableRecordEnvelope`] stored in
//! `echo/records/<kind>/<uuid-prefix>/<uuid>.json`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::Error;

use portable::{err, normalize_slash, validate_components, validate_relative, MAX_COMPONENT_BYTES};

// ── Layout constants ────────────────────────────────────────────────────────

/// The media tree root inside a portable library (`media/`).
pub const MEDIA_ROOT: &str = "media";

/// The control surface root (`echo/`).
pub const CONTROL_ROOT: &str = "echo";

/// The manifest path relative to the library root (`echo/manifest.json`).
pub const MANIFEST_PATH: &str = "echo/manifest.json";

/// The records tree root (`echo/records/`).
pub const RECORDS_ROOT: &str = "echo/records";

/// The staging directory root (`echo/tmp/`), local-only and never synced.
pub const STAGING_ROOT: &str = "echo/tmp";

/// Current format version for [`LibraryManifest`].
pub const CURRENT_FORMAT_VERSION: u64 = 2;

/// Maximum depth for object record paths inside `echo/records/<kind>/` to
/// prevent pathological directory nesting.  The real layout is at most
/// `kind/prefix/uuid.json` (depth 3).
pub const MAX_RECORD_DEPTH: usize = 8;

// ── Path types ──────────────────────────────────────────────────────────────

/// A validated, normalized path within the portable `echo/` control surface.
///
/// Starts with `echo/` or `echo/<subdir>/`, never with `media/`, is always
/// relative to the library root, never contains `..`, NUL, or control
/// characters.
///
/// Paths resolve inside `echo/` but **never** inside `echo/tmp/` (staging is
/// local-only and never enters portable objects).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ControlPath {
    normalized: String,
}

impl ControlPath {
    /// Construct and validate a control surface path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the value is not a safe, normalized
    /// control surface path (not under `echo/`, under `echo/tmp/`, absolute,
    /// escaping `..`, NUL, control characters, empty, or redundant components).
    pub fn new(value: &str) -> Result<Self, Error> {
        Self::validate(value)?;
        Ok(Self {
            normalized: normalize_slash(value),
        })
    }

    /// Validate without constructing (cheap guard for boundary checks).
    ///
    /// # Errors
    ///
    /// Same failure set as [`Self::new`].
    pub fn validate(value: &str) -> Result<(), Error> {
        validate_relative(value)?;
        let normalized = normalize_slash(value);
        if normalized.is_empty() {
            return Err(err("path is empty"));
        }
        if !normalized.starts_with("echo/") {
            return Err(err("path must start with 'echo/'"));
        }
        if normalized == "echo" {
            return Err(err("path must not be the bare 'echo' directory"));
        }
        if normalized.starts_with("echo/tmp/") || normalized == "echo/tmp" {
            return Err(err("path must not reference echo/tmp/ staging directory"));
        }
        // Control paths may not directly reference a media file.
        if normalized.starts_with("media/") {
            return Err(err("control path must not reference media/ tree"));
        }
        if normalized.len() > MAX_COMPONENT_BYTES {
            return Err(err("control path exceeds byte limit"));
        }
        // Reject `..`/`.`/empty/trailing components like any validated path.
        validate_components(&normalized, MAX_RECORD_DEPTH, MAX_COMPONENT_BYTES)?;
        Ok(())
    }

    /// The normalised, `/`-separated form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }
}

impl fmt::Display for ControlPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl std::str::FromStr for ControlPath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// A validated media path within the library root.
///
/// New Echo imports use the canonical `media/` tree, while existing libraries
/// may contain media elsewhere under the root. This type accepts both forms
/// but reserves the `echo/` control surface and rejects unsafe paths.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct LibraryRelativePath {
    normalized: String,
}

impl LibraryRelativePath {
    /// Maximum number of path components.
    pub const MAX_COMPONENTS: usize = 64;
    /// Maximum bytes per component (UTF-8).
    pub const MAX_COMPONENT_BYTES: usize = 4096;

    /// Construct and validate a library-relative media path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the value is empty, uses the `echo/`
    /// control surface, is absolute, escapes with `..`, contains
    /// NUL/control characters, or has empty/redundant components.
    pub fn new(value: &str) -> Result<Self, Error> {
        Self::validate(value)?;
        Ok(Self {
            normalized: normalize_slash(value),
        })
    }

    /// Validate without constructing.
    ///
    /// # Errors
    ///
    /// Same failure set as [`Self::new`].
    pub fn validate(value: &str) -> Result<(), Error> {
        validate_relative(value)?;
        let normalized = normalize_slash(value);
        if normalized.is_empty() {
            return Err(err("path is empty"));
        }
        // `echo/` contains manifests and sync records. Never let a song path
        // address that control surface, including on case-insensitive disks.
        if normalized
            .split('/')
            .next()
            .is_some_and(|root| root.eq_ignore_ascii_case("echo"))
        {
            return Err(err(
                "media path must not use the reserved 'echo/' directory",
            ));
        }
        validate_components(&normalized, Self::MAX_COMPONENTS, Self::MAX_COMPONENT_BYTES)?;
        Ok(())
    }

    /// The normalised, `/`-separated form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }
}

impl fmt::Display for LibraryRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl std::str::FromStr for LibraryRelativePath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

// ── Layout validation ───────────────────────────────────────────────────────

/// Rules for a valid portable library layout.
pub trait ValidateLibraryLayout {
    /// Validate that the library root follows the portable layout convention.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the layout violates portable
    /// constraints (missing `media/`, orphan `echo/tmp/` as media, etc.).
    fn validate_layout_rules(root: &str) -> Result<(), Error>;
}

/// Default implementation of layout validation rules.
pub struct DefaultLayoutValidator;

impl ValidateLibraryLayout for DefaultLayoutValidator {
    fn validate_layout_rules(root: &str) -> Result<(), Error> {
        let root = normalize_slash(root);
        if root.is_empty() {
            return Ok(());
        }

        // Reject media path under echo/ control surface.
        if root.starts_with("echo/") || root == "echo" {
            return Err(err(
                "media must not be placed under the echo/ control surface",
            ));
        }

        // Reject bare echo/tmp reference.
        if root == STAGING_ROOT || root.starts_with("echo/tmp/") {
            return Err(err(
                "echo/tmp/ is a local staging directory and not part of the portable layout",
            ));
        }

        Ok(())
    }
}

// ── Manifest ────────────────────────────────────────────────────────────────

/// The library manifest persisted at `echo/manifest.json`.
///
/// Contains the library's stable identity and format metadata.  A new device
/// reads this first, verifies format compatibility, then pulls object records
/// and scans `media/`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LibraryManifest {
    /// Schema version for backward compatibility checks.
    pub format_version: u64,
    /// The stable, globally unique identity of this library.
    pub library_id: LibraryId,
    /// Application version that last wrote this manifest.
    pub written_by_app_version: String,
}

impl LibraryManifest {
    /// Create a new manifest for a freshly initialized library.
    #[must_use]
    pub fn new(library_id: LibraryId, app_version: impl Into<String>) -> Self {
        Self {
            format_version: CURRENT_FORMAT_VERSION,
            library_id,
            written_by_app_version: app_version.into(),
        }
    }

    /// Whether this manifest's format version is compatible with the current
    /// application.
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.format_version >= 1 && self.format_version <= CURRENT_FORMAT_VERSION
    }
}

// ── Identity newtypes & portable records live in submodules:
// - [`identity`] — `LibraryId`, `DeviceId`, `HybridLogicalClock`.
// - [`portable`] — `RecordKind` … `PortableSerialize` and the record JSON IO.

mod identity;
mod portable;

#[cfg(test)]
mod tests;

pub use identity::{next_hlc, DeviceId, HybridLogicalClock, LibraryId};
pub use portable::*;
