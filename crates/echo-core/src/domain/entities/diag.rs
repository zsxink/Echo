//! Per-file media diagnostics.

use crate::domain::ids::RelativeMediaPath;

// ---------------------------------------------------------------------------
// Media diagnostic
// ---------------------------------------------------------------------------

/// Per-file diagnostic produced by scan/import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaDiagnostic {
    path: RelativeMediaPath,
    /// Stable code (see the error classification for strings).
    code: &'static str,
    reason: String,
    /// Song may not have been created (e.g. probe failure).
    song_created: bool,
}

impl MediaDiagnostic {
    #[must_use]
    pub const fn new(
        path: RelativeMediaPath,
        code: &'static str,
        reason: String,
        song_created: bool,
    ) -> Self {
        Self {
            path,
            code,
            reason,
            song_created,
        }
    }

    #[must_use]
    pub const fn path(&self) -> &RelativeMediaPath {
        &self.path
    }
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
    #[must_use]
    pub const fn song_created(&self) -> bool {
        self.song_created
    }
}
