//! Import flows (task 7.3 / 11.7). The dialog runs entirely on the Rust/Tauri
//! boundary; the `WebView` only ever receives relative paths and user-safe
//! errors.

use echo_core::application::import::PlanImport;
use echo_core::error::Error;

use crate::ipc::dto::{ImportBatchDto, ImportResultDto};
use crate::platform::import::SingleFileImport;

impl super::AppServices {
    /// Run the desktop import-file dialog and import the chosen files into the
    /// active root. The dialog runs entirely on the Rust/Tauri boundary; the
    /// `WebView` only ever receives the per-input result DTO (relative paths and
    /// user-safe errors).
    ///
    /// Returns `Ok(None)` when the user **cancelled** the dialog — a cancel is
    /// a no-op, never a successful (or failed) batch.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or no root is active; only
    /// batch-level failures propagate — per-input problems become
    /// `ImportResultDto::Failed` so one bad file cannot abort the batch.
    pub fn choose_and_import_files(&self) -> Result<Option<ImportBatchDto>, Error> {
        self.guard_writes()?;
        let Some(picked) = self.dialogs.pick_audio_files()? else {
            // Cancelled: not an error, not a success — a genuine no-op.
            return Ok(None);
        };
        let root = self
            .deps
            .roots
            .active_root()?
            .map(|r| r.id())
            .ok_or_else(|| Error::unavailable("library", "no active library root"))?;
        if picked.sources.is_empty() {
            return Ok(Some(ImportBatchDto { results: vec![] }));
        }
        let report = PlanImport::new(self.deps.as_ref(), picked.reader.as_ref())
            .run(root, &picked.sources)?;
        Ok(Some(ImportBatchDto::from(report)))
    }

    /// Import a single file (by absolute, desktop-owned path) into the active
    /// library root (task 11.7: "import current temporary playback item").
    ///
    /// The path stays entirely desktop-side — it is never forwarded to the
    /// `WebView`. The caller (the Tauri command layer) extracts it from the
    /// coordinator's current queue entry and passes it here.
    ///
    /// Returns `Ok(result)` with the single-file import result (imported /
    /// duplicate / unsupported / failed) or `Err` when the root is unavailable
    /// or the path is unreadable.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled, no root is active, or the path
    /// is unreadable; infrastructure errors propagate; per-input problems
    /// become `ImportResultDto::Failed`.
    pub fn import_single_path(
        &self,
        absolute_path: &std::path::Path,
        display_name: &str,
    ) -> Result<ImportResultDto, Error> {
        self.guard_writes()?;
        let root_id = self
            .deps
            .roots
            .active_root()?
            .map(|r| r.id())
            .ok_or_else(|| Error::unavailable("library", "no active library root"))?;
        let reader = SingleFileImport::new(absolute_path, display_name)?;
        let source = reader.source().clone();
        let report = PlanImport::new(self.deps.as_ref(), &reader).run(root_id, &[source])?;
        // A single-source batch always has exactly one result.
        let outcome = report
            .results
            .into_iter()
            .next()
            .expect("import batch has at least one result");
        Ok(ImportResultDto::from(outcome))
    }
}
