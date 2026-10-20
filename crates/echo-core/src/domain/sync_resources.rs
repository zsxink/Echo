//! Typed, path-safe manifests for immutable shared library resources.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use super::library::LibraryRelativePath;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedResourceKind {
    Audio,
    LyricsSidecar,
    UserCover,
    PlaylistCover,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceDigest([u8; 32]);

impl ResourceDigest {
    #[must_use]
    pub const fn from_bytes(value: [u8; 32]) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
    #[must_use]
    pub fn to_hex(self) -> String {
        self.0
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
                hex
            })
    }
}

/// Library-authorized immutable resource metadata. It contains no absolute
/// path, user credential or machine-local cache key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SharedResourceManifest {
    pub kind: SharedResourceKind,
    pub digest: ResourceDigest,
    pub length: u64,
    pub media_type: String,
    pub relative_path: LibraryRelativePath,
}

impl SharedResourceManifest {
    /// Validate the resource media type and library relative path.
    ///
    /// # Errors
    /// Returns a validation error for an invalid media type or unsafe path.
    pub fn new(
        kind: SharedResourceKind,
        digest: ResourceDigest,
        length: u64,
        media_type: String,
        relative_path: &str,
    ) -> Result<Self, crate::error::Error> {
        if !valid_media_type(&media_type) {
            return Err(crate::error::Error::validation(
                crate::error::Subject::Other,
                "resource media type",
                "invalid media type",
            ));
        }
        Ok(Self {
            kind,
            digest,
            length,
            media_type,
            relative_path: LibraryRelativePath::new(relative_path)?,
        })
    }
}

fn valid_media_type(value: &str) -> bool {
    if value.is_empty() || value.len() > 127 {
        return false;
    }
    let Some((major, minor)) = value.split_once('/') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.bytes().chain(minor.bytes()).all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}
