//! Reveal-in-folder fallback policy (task 9.5).
//!
//! A reveal request (`reveal_song`) resolves a `SongId` to an absolute
//! location only inside the platform boundary and asks the OS file manager to
//! show it. On Linux a file manager may be unable to select the specific row
//! (Nautilus/`dolphin` variants); the platform then falls back to opening the
//! **parent directory**, which still satisfies "reveal" — it is never reported
//! as `Unavailable` unless every path fails.
//!
//! This module owns that decision as a pure policy the composition root's real
//! reveal backend consumes; the entry point keeps taking `SongId`/relative
//! paths (the `AppServices::reveal_song` boundary), never an arbitrary path.

/// The outcome of one OS "reveal a path" attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendReveal {
    /// The file manager selected the specific file.
    RevealedFile,
    /// The platform cannot select the specific row (some Linux file managers);
    /// opening the parent directory still satisfies the reveal.
    NotLocatable,
    /// Reveal failed outright — no file manager, or the path vanished.
    Failed,
}

/// The reveal target a backend outcome maps to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevealTarget {
    /// Open the file manager showing the specific file.
    TheFile,
    /// Open the file manager on the file's parent directory.
    ParentDirectory,
}

/// Decide what to reveal given a backend outcome.
///
/// - `RevealedFile` → reveal the file itself.
/// - `NotLocatable` → fall back to the parent directory (Linux cannot select a
///   row), which still counts as a reveal — **never** an `Unavailable`.
/// - `Failed` → no target; the caller reports the relative path instead (the
///   existing soft-failure path in `AppServices::reveal_song`).
#[must_use]
pub const fn target_for(outcome: BackendReveal) -> Option<RevealTarget> {
    match outcome {
        BackendReveal::RevealedFile => Some(RevealTarget::TheFile),
        BackendReveal::NotLocatable => Some(RevealTarget::ParentDirectory),
        BackendReveal::Failed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_file_reveal_targets_the_file() {
        assert_eq!(
            target_for(BackendReveal::RevealedFile),
            Some(RevealTarget::TheFile)
        );
    }

    #[test]
    fn not_locatable_falls_back_to_the_parent_directory_not_unavailable() {
        // The core of "Linux 无法定位时打开父目录": a non-locatable row is not
        // a failure — it opens the parent directory and stays Revealed.
        assert_eq!(
            target_for(BackendReveal::NotLocatable),
            Some(RevealTarget::ParentDirectory)
        );
    }

    #[test]
    fn a_failed_reveal_has_no_target() {
        assert_eq!(target_for(BackendReveal::Failed), None);
    }
}
