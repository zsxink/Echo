//! Desktop file/directory pickers and reveal-in-folder port (task 7.5, design §10).
//!
//! The `WebView` never receives a general filesystem capability. Directory and
//! import-file selection happens on the **Rust/Tauri dialog boundary**: a dialog
//! yields logical handles (never absolute paths) that are handed straight to a
//! core use case, and a cancelled dialog returns `Ok(None)` — never a fake
//! success. `reveal` takes a library-relative path and performs the OS
//! reveal-in-folder as a desktop side effect; it never returns the absolute
//! location to the caller.
//!
//! The live implementation (behind the Tauri shell) is wired where the shell
//! owns its `tauri` + dialog dependencies; `echo-desktop` stays verbatim
//! Tauri-free, so these rules are proven against [`TestDialogs`] without any
//! window or plugin.

use echo_core::application::ports::{ImportSource, ImportSourceReader};
use echo_core::error::Error;

/// The outcome of a reveal-in-folder side effect. Callers render only
/// `SongId`/relative-path data; the absolute location never crosses back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevealOutcome {
    /// The file manager is showing the file (or its parent directory when the
    /// platform cannot select a specific row).
    Revealed,
    /// The path could not be revealed (no file manager, vanished file, …).
    /// This is a soft failure: the caller shows the relative path instead.
    Unavailable,
}

/// One dialog's accepted file selection: logical source handles plus the
/// reader that resolves them to real content within the trusted boundary.
pub struct PickedImport {
    /// Logical, non-path handles Core's import use case consumes.
    pub sources: Vec<ImportSource>,
    /// Resolves each handle to real content. Owned beside the handles so a
    /// selection and its reader can never drift apart.
    pub reader: Box<dyn ImportSourceReader>,
}

/// The desktop picker/reveal boundary (design §10). `None` always means the
/// user cancelled the dialog — a cancelled dialog is not an error and is never
/// reported as a success.
pub trait SystemDialogs: Send + Sync {
    /// Pick a directory to use as the library root. `Ok(None)` = cancelled.
    ///
    /// # Errors
    ///
    /// Returns the dialog boundary's error when the picker itself failed (not
    /// when the user cancelled — that is `Ok(None)`).
    fn pick_library_directory(&self) -> Result<Option<std::path::PathBuf>, Error>;

    /// Pick one or more audio files to import. `Ok(None)` = cancelled.
    ///
    /// # Errors
    ///
    /// Returns the dialog boundary's error when the picker itself failed (not
    /// when the user cancelled — that is `Ok(None)`).
    fn pick_audio_files(&self) -> Result<Option<PickedImport>, Error>;

    /// Reveal the file at `absolute` in the OS file manager. `absolute` is the
    /// resolved desktop-side location (root + relative joined); it is consumed
    /// here and never returned.
    ///
    /// # Errors
    ///
    /// Returns the dialog boundary's error when the reveal call itself failed
    /// (a soft failure is reported as [`RevealOutcome::Unavailable`], not an
    /// `Err`).
    fn reveal(&self, absolute: &std::path::Path) -> Result<RevealOutcome, Error>;
}

/// A deterministic dialog double recording fixed picks and reveal markers.
pub struct TestDialogs {
    directory: std::sync::Mutex<Option<std::path::PathBuf>>,
    pick: std::sync::Mutex<Option<PickedImport>>,
    revealed: std::sync::Mutex<Vec<String>>,
    reveal_outcome: RevealOutcome,
}

impl TestDialogs {
    /// A double that always cancels both pickers and always succeeds reveal.
    #[must_use]
    pub const fn cancelling() -> Self {
        Self {
            directory: std::sync::Mutex::new(None),
            pick: std::sync::Mutex::new(None),
            revealed: std::sync::Mutex::new(Vec::new()),
            reveal_outcome: RevealOutcome::Revealed,
        }
    }

    /// A double that returns `directory` from the directory picker.
    #[must_use]
    pub const fn with_directory(directory: Option<std::path::PathBuf>) -> Self {
        Self {
            directory: std::sync::Mutex::new(directory),
            pick: std::sync::Mutex::new(None),
            revealed: std::sync::Mutex::new(Vec::new()),
            reveal_outcome: RevealOutcome::Revealed,
        }
    }

    /// The relative locate-markers passed to `reveal`, in order (for asserting
    /// the caller revealed by `SongId` without the dialog leaking a path).
    pub fn revealed(&self) -> Vec<String> {
        self.revealed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl SystemDialogs for TestDialogs {
    fn pick_library_directory(&self) -> Result<Option<std::path::PathBuf>, Error> {
        let mut slot = self
            .directory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(slot.take())
    }

    fn pick_audio_files(&self) -> Result<Option<PickedImport>, Error> {
        // Take the configured pick out (once); cancellation is `Ok(None)`.
        let mut slot = self
            .pick
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(slot.take())
    }

    fn reveal(&self, absolute: &std::path::Path) -> Result<RevealOutcome, Error> {
        // Record only the file name (never the absolute path) so tests can
        // assert reveal-by-SongId happened without the location leaking.
        let marker = absolute
            .file_name()
            .map_or_else(|| "<none>".to_owned(), |n| n.to_string_lossy().to_string());
        self.revealed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(marker);
        Ok(self.reveal_outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_picker_is_ok_none_not_success() {
        let dialogs = TestDialogs::cancelling();
        // The command sees "no selection", which it maps to "no-op", not a
        // successful import — asserted at the service layer; here we only
        // prove the double's contract shape.
        assert!(dialogs
            .pick_audio_files()
            .expect("dialog is not an error")
            .is_none());
        // The directory picker cancels the same way: `Ok(None)`, never an
        // empty directory that gets interpreted as a successful activation.
        assert!(dialogs
            .pick_library_directory()
            .expect("dialog is not an error")
            .is_none());
    }

    #[test]
    fn reveal_records_a_relative_marker_not_the_absolute_path() {
        let dialogs = TestDialogs::cancelling();
        let outcome = dialogs
            .reveal(std::path::Path::new("/Users/me/Music/晴天.flac"))
            .expect("reveal");
        assert_eq!(outcome, RevealOutcome::Revealed);
        assert_eq!(dialogs.revealed(), vec!["晴天.flac".to_owned()]);
        // No test observes an absolute path — the port consumes it.
    }
}
