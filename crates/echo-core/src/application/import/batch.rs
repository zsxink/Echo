use crate::application::ports::{ImportSource, ImportSourceInfo, ImportSourceReader};
use crate::application::scan::ScanDeps;
use crate::domain::entities::SongAvailability;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, SongId};
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
        let started = std::time::Instant::now();
        if self.deps.roots.replica_enrollment_pending(root)?
            || self
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
        super::trace_import_stage(root, "import_validate_root", started);
        let started = std::time::Instant::now();
        if crate::application::scan::durable::library_id(self.deps, root).is_err() {
            return Ok(ImportBatchReport {
                results: vec![ImportOutcome::LibraryUnavailable; sources.len()],
            });
        }
        super::trace_import_stage(root, "import_read_library_identity", started);
        let started = std::time::Instant::now();
        let mut state = self.batch_state(root)?;
        super::trace_import_stage(root, "import_build_batch_state", started);
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
        let started = std::time::Instant::now();
        let durable = self.deps.control.durability().ok_or_else(|| {
            Error::unavailable(
                "library durability",
                "directory commit adapter is not configured",
            )
        })?;
        let replay = durable.projection_replay(root)?;
        let indexed_snapshot_current =
            indexed_snapshot_is_current(self.deps.history.as_deref(), root, &replay);
        let songs = if indexed_snapshot_current {
            super::trace_import_stage(root, "import_open_journal_snapshot", started);
            let started = std::time::Instant::now();
            let songs = self.deps.songs.all_in_root(root)?;
            super::trace_import_stage(root, "import_indexed_song_snapshot", started);
            songs
        } else {
            let started = std::time::Instant::now();
            let songs = crate::application::scan::durable::song_snapshot(self.deps, root)?;
            super::trace_import_stage(root, "import_snapshot_songs", started);
            songs
        };
        for song in songs {
            // Missing and Echo-pending-delete records remain in the repository
            // for UUID/recovery semantics, but they no longer represent a
            // playable catalogue holder or a logical target claimed by the
            // current import. Actual files on disk are still reserved below.
            if song.availability() != SongAvailability::Available {
                // An authoritative identity may be waiting for its SQLite
                // projection. Only matching, present bytes own import content.
                if self.deps.hasher.hash(root, song.path()).ok().as_deref() != song.blake3_hash() {
                    continue;
                }
            }
            if let Some(hash) = song.blake3_hash() {
                conflicts.record_content(hash, song.id());
            }
            conflicts.occupy_target(song.path().identity_key());
        }
        if indexed_snapshot_current {
            let mut claims = std::collections::BTreeMap::new();
            for state in replay.states() {
                if state.kind == crate::domain::library::durability::StateKind::Claim
                    && state.payload["kind"] == "ingestion"
                {
                    claims.insert(state.id, state.payload.clone());
                }
            }
            let mut claimed_songs = Vec::new();
            let mut claimed_paths = Vec::new();
            for claim in claims.into_values().filter(|claim| claim["active"] == true) {
                let (Some(target), Some(hash), Some(song)) = (
                    claim["target"].as_str(),
                    claim["content_hash"].as_str(),
                    claim["song_uuid"].as_str(),
                ) else {
                    continue;
                };
                let path = RelativeMediaPath::new(target)?;
                let song = song
                    .parse::<SongId>()
                    .map_err(|_| Error::conflict("invalid durable import identity"))?;
                if claim["added_at"].as_u64().is_none() {
                    return Err(Error::conflict("durable import has no addition time"));
                }
                if claimed_songs.contains(&song)
                    || claimed_paths.contains(&path)
                    || self.deps.hasher.hash(root, &path).ok().as_deref() != Some(hash)
                {
                    continue;
                }
                conflicts.record_content(hash.to_owned(), song);
                conflicts.occupy_target(path.identity_key());
                claimed_songs.push(song);
                claimed_paths.push(path);
            }
        }
        let started = std::time::Instant::now();
        for path in self.deps.fs.enumerate(root)? {
            conflicts.occupy_target(path.identity_key());
        }
        super::trace_import_stage(root, "import_enumerate_files", started);
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
        let plan_started = std::time::Instant::now();
        let planned = match self.stage_and_plan(root, source, &info, &ext, state) {
            Ok(planned) => planned,
            Err(outcome) => return *outcome,
        };
        super::trace_import_stage(root, "stage_and_plan", plan_started);
        // Persist the intent (reserved SongId + per-resource source/staging/
        // target/hash + the conditional unique target claim) before the first
        // external side effect; the claim keeps the target path reserved until
        // the operation reaches a terminal state. The audio is the main
        // resource; a same-basename `.lrc` is the same journal's optional
        // sub-resource with its own item row, target claim and hash (design
        // §8: item 固定保存 kind(audio|lrc)、源定位、暂存/目标、预期 BLAKE3).
        let journal_started = std::time::Instant::now();
        let started = std::time::Instant::now();
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
        super::trace_import_stage(root, "import_journal_ensure_operation", started);
        let started = std::time::Instant::now();
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
        super::trace_import_stage(root, "import_journal_audio_planned", started);
        if let Some(lrc) = &planned.lrc {
            let started = std::time::Instant::now();
            if let Err(error) = self.deps.journal.upsert_item(
                planned.operation,
                journal_lrc_item(OperationState::Planned, &planned, lrc),
            ) {
                let _ = self.deps.fs.discard_staged(root, &planned.staged);
                let _ = self.deps.fs.discard_staged(root, &lrc.staged);
                return failed_of(&error);
            }
            super::trace_import_stage(root, "import_journal_lyrics_planned", started);
        }
        state.conflicts.occupy_target(planned.target.identity_key());
        if let Some(lrc) = &planned.lrc {
            state.conflicts.occupy_target(lrc.target.identity_key());
        }

        let started = std::time::Instant::now();
        if let Err(error) = self.persist_reservation(&planned) {
            return self.recover_execution_failure(root, &planned, ExecuteError::PrePublish(error));
        }
        super::trace_import_stage(root, "import_persist_reservation", started);
        super::trace_import_stage(root, "journal_plan_and_reservation", journal_started);
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
            Err(ExecuteError::SavedPending { error, lyrics }) => {
                state
                    .conflicts
                    .record_import(planned.hash.clone(), planned.reserved);
                ImportOutcome::SavedPending {
                    operation: planned.operation,
                    song: planned.reserved,
                    target: planned.target.clone(),
                    renamed: planned.renamed,
                    lyrics: Box::new(lyrics),
                    code: error.code(),
                    message: error.to_string(),
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
            ExecuteError::SavedPending { error, lyrics } => ImportOutcome::SavedPending {
                operation: planned.operation,
                song: planned.reserved,
                target: planned.target.clone(),
                renamed: planned.renamed,
                lyrics: Box::new(lyrics),
                code: error.code(),
                message: error.to_string(),
            },
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

fn indexed_snapshot_is_current(
    history: Option<&dyn crate::application::ports::LibraryHistoryPort>,
    root: LibraryRootId,
    replay: &crate::domain::library::durability::LibraryReplay,
) -> bool {
    history.is_some_and(|history| match history.projection_status(root) {
        Ok(status) => {
            let expected = replay.head();
            let matches_head = expected.map_or_else(
                || {
                    status.applied_library_commit_seq == 0
                        && status.applied_head_digest.is_none()
                },
                |(sequence, digest)| {
                    status.applied_library_commit_seq == sequence
                        && status.applied_head_digest.as_deref() == Some(digest)
                },
            );
            let unresolved_song_record = status.unresolved.iter().any(|record| {
                matches!(
                    &record.target,
                    crate::domain::library::durability::ProjectionTarget::Record {
                        object: crate::domain::sync::ObjectKey::Song(_)
                    }
                )
            });
            if std::env::var_os("ECHO_IMPORT_TRACE").is_some() {
                eprintln!(
                    "ECHO_IMPORT_TRACE root={root} stage=import_projection_head current={matches_head} expected={expected:?} actual=({}, {:?}) unresolved={} unresolved_song_record={unresolved_song_record}",
                    status.applied_library_commit_seq,
                    status.applied_head_digest,
                    status.unresolved.len(),
                );
            }
            matches_head && !unresolved_song_record
        }
        Err(_) => false,
    })
}
