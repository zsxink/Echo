use crate::application::ports::{ImportSource, ImportSourceInfo, ImportSourceReader};
use crate::application::scan::ScanDeps;
use crate::domain::entities::SongAvailability;
use crate::domain::ids::LibraryRootId;
use crate::domain::import::ImportConflictIndex;
use crate::domain::state::OperationState;
use crate::error::Error;

use super::report::{failed_of, journal_item, journal_lrc_item, supported_extension_of};
use super::{
    BatchState, ExecuteError, ImportBatchReport, ImportOutcome, PlanImport, PlannedInput,
    IMPORT_OPERATION,
};

impl<'a> PlanImport<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, sources: &'a dyn ImportSourceReader) -> Self {
        Self { deps, sources }
    }

    /// Run one batch against `root`.
    ///
    /// # Errors
    ///
    /// Only batch-level infrastructure failures (root-capability or library
    /// snapshot reads) propagate; per-input problems become `Failed` results
    /// so one bad input can never abort the batch.
    pub fn run(
        &self,
        root: LibraryRootId,
        sources: &[ImportSource],
    ) -> Result<ImportBatchReport, Error> {
        // Refuse the whole batch before any copy when the root cannot take
        // writes (spec: 资料库根目录不可用时在开始复制前拒绝整批导入).
        if self
            .deps
            .roots
            .by_id(root)?
            .is_some_and(|record| !record.write_capable())
            || !self.deps.fs.write_capable(root)?
        {
            return Ok(ImportBatchReport {
                results: vec![ImportOutcome::LibraryUnavailable; sources.len()],
            });
        }
        let mut state = self.batch_state(root)?;
        let mut results = Vec::with_capacity(sources.len());
        for source in sources {
            results.push(self.import_one(root, source, &mut state));
        }
        Ok(ImportBatchReport { results })
    }

    /// The batch-start identity snapshot: every hash holder, plus every
    /// occupied target path — library records *and* files that exist only on
    /// disk (external placement, dropped database), so numbering continues
    /// until the target truly does not exist (绝不覆盖既有文件).
    fn batch_state(&self, root: LibraryRootId) -> Result<BatchState, Error> {
        let mut conflicts = ImportConflictIndex::default();
        for song in self.deps.songs.all_in_root(root)? {
            // Missing and Echo-pending-delete records remain in the repository
            // for UUID/recovery semantics, but they no longer represent a
            // playable catalogue holder or a logical target claimed by the
            // current import. Actual files on disk are still reserved below.
            if song.availability() != SongAvailability::Available {
                continue;
            }
            if let Some(hash) = song.blake3_hash() {
                conflicts.record_content(hash, song.id());
            }
            conflicts.occupy_target(song.path().identity_key());
        }
        for path in self.deps.fs.enumerate(root)? {
            conflicts.occupy_target(path.identity_key());
        }
        Ok(BatchState { conflicts })
    }

    /// Classify, plan and import one input. Never fails the batch: every
    /// failure mode becomes this input's `Failed` result (and rolls back its
    /// own journal claim and staged copy) while the other inputs continue.
    fn import_one(
        &self,
        root: LibraryRootId,
        source: &ImportSource,
        state: &mut BatchState,
    ) -> ImportOutcome {
        let info: ImportSourceInfo = match self.sources.describe(source) {
            Ok(info) => info,
            Err(error) => return failed_of(&error),
        };
        // The extension is both the type filter and the target's 原扩展名
        // (normalized to lowercase for a deterministic target name). A non-
        // audio input in a multi-select is a benign skip, not a failure.
        let Some(ext) = supported_extension_of(&info.display_name) else {
            return ImportOutcome::Skipped;
        };
        let planned = match self.stage_and_plan(root, source, &info, &ext, state) {
            Ok(planned) => planned,
            Err(outcome) => return outcome,
        };
        // Persist the intent (reserved SongId + per-resource source/staging/
        // target/hash + the conditional unique target claim) before the first
        // external side effect; the claim keeps the target path reserved until
        // the operation reaches a terminal state. The audio is the main
        // resource; a same-basename `.lrc` is the same journal's optional
        // sub-resource with its own item row, target claim and hash (design
        // §8: item 固定保存 kind(audio|lrc)、源定位、暂存/目标、预期 BLAKE3).
        if let Err(error) = self.deps.journal.ensure_operation(
            planned.operation,
            root,
            IMPORT_OPERATION,
            Some(planned.reserved),
        ) {
            let _ = self.deps.fs.discard_staged(root, &planned.staged);
            let _ = planned
                .lrc
                .as_ref()
                .map(|lrc| self.deps.fs.discard_staged(root, &lrc.staged));
            return failed_of(&error);
        }
        if let Err(error) = self.deps.journal.upsert_item(
            planned.operation,
            journal_item(OperationState::Planned, &planned),
        ) {
            let _ = self.deps.fs.discard_staged(root, &planned.staged);
            let _ = planned
                .lrc
                .as_ref()
                .map(|lrc| self.deps.fs.discard_staged(root, &lrc.staged));
            return failed_of(&error);
        }
        if let Some(lrc) = &planned.lrc {
            if let Err(error) = self.deps.journal.upsert_item(
                planned.operation,
                journal_lrc_item(OperationState::Planned, &planned, lrc),
            ) {
                let _ = self.deps.fs.discard_staged(root, &planned.staged);
                let _ = self.deps.fs.discard_staged(root, &lrc.staged);
                return failed_of(&error);
            }
        }
        state.conflicts.occupy_target(planned.target.identity_key());
        if let Some(lrc) = &planned.lrc {
            state.conflicts.occupy_target(lrc.target.identity_key());
        }

        match self.execute(&planned) {
            Ok(lyrics) => {
                state
                    .conflicts
                    .record_import(planned.hash.clone(), planned.reserved);
                ImportOutcome::Imported {
                    operation: planned.operation,
                    song: planned.reserved,
                    target: planned.target.clone(),
                    renamed: planned.renamed,
                    lyrics: Box::new(lyrics),
                }
            }
            Err(error) => self.recover_execution_failure(root, &planned, error),
        }
    }

    /// Handle an [`ExecuteError`]: a failure before any publish is safe to roll
    /// back (terminal `RolledBack` + release the claim + discard staged copies),
    /// while a failure at/after the publish must NOT roll back — a published
    /// final file (or an in-flight DB commit) is left recoverable and its
    /// target claim is held so recovery completes the same operation under the
    /// same reserved identity, never orphaning the file or creating a second
    /// UUID (task 5.5).
    fn recover_execution_failure(
        &self,
        root: LibraryRootId,
        planned: &PlannedInput,
        error: ExecuteError,
    ) -> ImportOutcome {
        match error {
            ExecuteError::PrePublish(error) => {
                // Nothing published: the terminal state releases this input's
                // claims; committed inputs and the remaining batch are
                // untouched. Both staged copies are cleaned up best-effort.
                let _ = self.deps.fs.discard_staged(root, &planned.staged);
                let _ = planned
                    .lrc
                    .as_ref()
                    .map(|lrc| self.deps.fs.discard_staged(root, &lrc.staged));
                let _ = self.deps.journal.upsert_item(
                    planned.operation,
                    journal_item(OperationState::RolledBack, planned),
                );
                if let Some(lrc) = &planned.lrc {
                    let _ = self.deps.journal.upsert_item(
                        planned.operation,
                        journal_lrc_item(OperationState::RolledBack, planned, lrc),
                    );
                }
                let _ = self.deps.journal.release_claims(planned.operation);
                failed_of(&error)
            }
            ExecuteError::PostPublish(error) => {
                // The audio (and possibly a sidecar) is already durably
                // published. Leave the item at its persisted state and hold the
                // claim: recovery — not a rollback — completes the record under
                // the reserved UUID. No discard, no release, no second copy.
                failed_of(&error)
            }
            ExecuteError::Duplicate { existing } => {
                // The pre-commit dedup found the content we just published is
                // already the library's under another UUID (task 5.6: 并发导入/
                // 监听竞态下相同内容只出现一个逻辑歌曲). Contribute nothing: remove
                // our own redundant published duplicate (guard: only if the file
                // at target still carries OUR content — never delete a foreign
                // file), roll both journal items back, release the claim, and
                // point the user at the existing record. No second UUID, no
                // duplicate file is left behind (绝不复制/绝无重复文件).
                if let Ok(published_hash) = self.deps.hasher.hash(planned.root, &planned.target) {
                    if published_hash == planned.hash {
                        let _ = self
                            .deps
                            .fs
                            .discard_published(planned.root, &planned.target);
                    }
                }
                let _ = self.deps.journal.upsert_item(
                    planned.operation,
                    journal_item(OperationState::RolledBack, planned),
                );
                if let Some(lrc) = &planned.lrc {
                    let _ = self.deps.journal.upsert_item(
                        planned.operation,
                        journal_lrc_item(OperationState::RolledBack, planned, lrc),
                    );
                }
                let _ = self.deps.journal.release_claims(planned.operation);
                ImportOutcome::Duplicate { existing }
            }
        }
    }
}
