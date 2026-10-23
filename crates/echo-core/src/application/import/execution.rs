use crate::application::ports::TxAccess;
use crate::application::relink::song_from_parsed;
use crate::application::scan::{
    parse_single_file_with_content_check, rewrap, FileOutcome, ParsedOutcome,
};
use crate::domain::entities::{LyricsSource, SongAvailability};
use crate::domain::ids::{LibraryRootId, SongId};
use crate::domain::state::OperationState;
use crate::error::Error;

use super::report::{journal_item, journal_lrc_item};
use super::{
    ExecuteError, LyricsFailure, LyricsImportResult, PlanImport, PlannedInput, PlannedLrc,
    IMPORT_OPERATION,
};

impl PlanImport<'_> {
    /// The directory reservation survives losing the entire application data
    /// directory between publication and the song commit.
    pub(super) fn persist_reservation(&self, planned: &PlannedInput) -> Result<(), Error> {
        use crate::domain::library::durability::{
            ChangeOrigin, CommitStatus, DurableStateChange, LibraryBatch, StateKind,
        };
        let library = crate::application::scan::durable::library_id(self.deps, planned.root)?;
        let operation = self.deps.ids.new_operation_id();
        let mut batch =
            LibraryBatch::new(operation.as_uuid(), library, ChangeOrigin::Import, vec![]);
        batch.state.push(DurableStateChange::new(StateKind::Claim, planned.operation.as_uuid(), serde_json::json!({
            "version": 1, "kind": "ingestion", "active": true,
            "song_uuid": planned.reserved.as_uuid(), "target": planned.target.display(),
            "content_hash": planned.hash, "byte_length": planned.size, "added_at": planned.added_at,
        })));
        let outcome = self
            .deps
            .control
            .durability()
            .ok_or_else(|| {
                Error::unavailable(
                    "library durability",
                    "directory commit adapter is not configured",
                )
            })?
            .commit(planned.root, &batch)?;
        if outcome.status == CommitStatus::SavedPendingRecovery {
            return Err(Error::saved_pending_recovery(
                operation,
                Error::unavailable("import reservation", "directory reservation needs recovery"),
            ));
        }
        Ok(())
    }
    fn write_state(&self, planned: &PlannedInput, state: OperationState) -> Result<(), Error> {
        self.deps
            .journal
            .upsert_item(planned.operation, journal_item(state, planned))
    }

    /// Verify the staged audio copy against the planned size/hash at the
    /// persisted staging location (the `ValidatePending → Validated` evidence;
    /// a truncated or corrupt staged copy is rejected here, never published).
    fn verify_staged(&self, planned: &PlannedInput) -> Result<(), Error> {
        let staging_path = &planned.staged_path;
        let meta = self.deps.fs.file_meta(planned.root, staging_path)?;
        if meta.size != planned.size {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "staged size differs from the copied content".to_owned(),
            });
        }
        let staged_hash = self.deps.hasher.hash(planned.root, staging_path)?;
        if staged_hash != planned.hash {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "staged hash differs from the copied content".to_owned(),
            });
        }
        Ok(())
    }

    /// The pre-commit half of the dual BLAKE3 dedup (task 5.6): after the final
    /// file is published and its full-file hash re-verified, check whether the
    /// library already owns a song with that exact content hash. A concurrent
    /// import or watcher may have committed the same content between the
    /// plan-time check and this commit — identical content must map to exactly
    /// one logical song, so a hit returns that existing UUID (excluding our own
    /// reserved one, which a racing watcher may already have applied under the
    /// reserved identity).
    fn pre_commit_duplicate(
        &self,
        root: LibraryRootId,
        published_hash: &str,
        reserved: SongId,
    ) -> Result<Option<SongId>, Error> {
        for song in crate::application::scan::durable::song_snapshot(self.deps, root)? {
            // A pending-delete or externally-missing row is retained for
            // recovery/relinking, but it is not a live content holder. The
            // final check must apply the same availability rule as planning,
            // otherwise a race-safe commit could resurrect Issue #12.
            if song.availability() != SongAvailability::Available {
                continue;
            }
            if song.blake3_hash() == Some(published_hash) && song.id() != reserved {
                return Ok(Some(song.id()));
            }
        }
        Ok(None)
    }

    /// Publish → verify → parse → commit one planned input, driving the audio
    /// journal item through the full per-resource `Copy/Validate/Publish
    /// Pending→Applied` chain (task 5.5). Returns the sidecar result: a
    /// same-basename `.lrc` is published *after* the audio is whole
    /// (best-effort — a sidecar failure becomes "audio succeeded / lyrics
    /// failed" and never rolls back the verified audio, design §8).
    ///
    /// Failures are classified by [`ExecuteError`]: before the publish they are
    /// safely rollable; at/after the publish the item is left recoverable so
    /// recovery completes it under the same reserved identity (a published
    /// final file is never rolled back into an orphan, and the target claim is
    /// never released early).
    #[allow(clippy::too_many_lines)] // one cohesive publish→verify→dedup→commit chain
    pub(super) fn execute(
        &self,
        planned: &PlannedInput,
    ) -> Result<LyricsImportResult, ExecuteError> {
        // Per-resource journal chain (design §8: 意图先持久化，副作用后校验再
        // 落 Applied). The staged copy was created and fsynced in pre-flight;
        // these intents/results make the chain durable and drive recovery.
        self.write_state(planned, OperationState::CopyPending)
            .map_err(ExecuteError::PrePublish)?;
        self.write_state(planned, OperationState::CopyApplied)
            .map_err(ExecuteError::PrePublish)?;
        self.write_state(planned, OperationState::ValidatePending)
            .map_err(ExecuteError::PrePublish)?;
        self.verify_staged(planned)
            .map_err(ExecuteError::PrePublish)?;
        self.write_state(planned, OperationState::Validated)
            .map_err(ExecuteError::PrePublish)?;
        self.write_state(planned, OperationState::PublishPending)
            .map_err(ExecuteError::PrePublish)?;

        // Publish reserves the target exclusively (create-new) + same-filesystem
        // rename: an occupied target is a conflict, never a replacement
        // (绝不覆盖既有文件). A publish *error* means the rename never happened —
        // the adapter leaves no half file and no published target — so a failure
        // here is still PrePublish (safe to roll back and release the claim).
        // Only AFTER the rename succeeds does the failure boundary flip to
        // PostPublish (a published final file must be recovered, never rolled
        // back orphaned).
        self.deps
            .fs
            .publish(planned.root, &planned.staged, &planned.target)
            .map_err(ExecuteError::PrePublish)?;
        // Only once the FINAL location is verified (size + hash) does the item
        // count as published (design §8).
        let meta = self
            .deps
            .fs
            .file_meta(planned.root, &planned.target)
            .map_err(ExecuteError::PostPublish)?;
        if meta.size != planned.size {
            return Err(ExecuteError::PostPublish(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "published size differs from the staged content".to_owned(),
            }));
        }
        let published_hash = self
            .deps
            .hasher
            .hash(planned.root, &planned.target)
            .map_err(ExecuteError::PostPublish)?;
        if published_hash != planned.hash {
            return Err(ExecuteError::PostPublish(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "published hash differs from the staged content".to_owned(),
            }));
        }
        // Pre-commit BLAKE3 dedup re-check (task 5.6, design §8: 去重在计划时和
        // 提交前各检查一次): the plan-time check saw no holder, but between then
        // and this commit a concurrent import/watcher may have committed the
        // same content (possibly at another path). Re-query the library by the
        // full-file hash just verified at the final location: identical content
        // returns the existing record and never a second logical song. A query
        // failure is logged and treated as "no duplicate" — the file is whole
        // and valid, and failing the import here would orphan it.
        if let Some(existing) = self
            .pre_commit_duplicate(planned.root, &published_hash, planned.reserved)
            .map_err(ExecuteError::PostPublish)?
        {
            return Err(ExecuteError::Duplicate { existing });
        }
        self.write_state(planned, OperationState::PublishApplied)
            .map_err(ExecuteError::PostPublish)?;

        // Same-name `.lrc` sub-resource: publish it beside the (now whole)
        // audio. A failure here is NOT an audio failure — the LRC item rolls
        // back individually and the result carries the lyrics-failed status.
        let lrc_result = planned.lrc.as_ref().map_or_else(
            || {
                planned
                    .lrc_failure
                    .as_ref()
                    .map_or(LyricsImportResult::None, |failure| {
                        LyricsImportResult::Failed {
                            code: failure.code,
                            message: failure.message.clone(),
                        }
                    })
            },
            |lrc| match self.publish_lrc(planned, lrc) {
                Ok(()) => LyricsImportResult::Imported {
                    target: lrc.target.clone(),
                },
                Err(failure) => {
                    let _ = self.deps.fs.discard_staged(planned.root, &lrc.staged);
                    let _ = self.deps.journal.upsert_item(
                        planned.operation,
                        journal_lrc_item(OperationState::RolledBack, planned, lrc),
                    );
                    LyricsImportResult::Failed {
                        code: failure.code,
                        message: failure.message,
                    }
                }
            },
        );
        // Only our own published sidecar may back the song's sidecar source; a
        // failed import must not pick up whatever happens to sit at the target.
        let lrc_published = matches!(&lrc_result, LyricsImportResult::Imported { .. });
        // Parse the published file with the shared scan pipeline (probe, tags,
        // sidecar, cover) and commit the record under the RESERVED identity.
        match parse_single_file_with_content_check(self.deps, planned.root, &planned.target, true) {
            FileOutcome::Parsed(parsed) => {
                if let Err(error) = self.commit(planned, &parsed, lrc_published) {
                    return Err(if matches!(error, Error::LibrarySavedPending { .. }) {
                        ExecuteError::SavedPending {
                            error,
                            lyrics: lrc_result,
                        }
                    } else {
                        ExecuteError::PostPublish(error)
                    });
                }
                Ok(lrc_result)
            }
            FileOutcome::Diagnostic(diagnostic) => {
                Err(ExecuteError::PostPublish(Error::CorruptMedia {
                    operation: IMPORT_OPERATION.to_owned(),
                    reason: diagnostic.reason().to_owned(),
                }))
            }
            FileOutcome::FastSkip { .. } => {
                Err(ExecuteError::PostPublish(Error::InvariantViolation {
                    why: "import target is already owned by another record".to_owned(),
                }))
            }
        }
    }

    /// Publish one sidecar resource with the same per-resource
    /// `Copy/Validate/Publish Pending→Applied` journal chain as the audio
    /// (task 5.5), then verify its final size/hash and record the per-resource
    /// `Completed` row. A failure is a [`LyricsFailure`] (never an audio
    /// failure); the exclusive publish guarantees no half sidecar can appear
    /// at the target.
    fn publish_lrc(&self, planned: &PlannedInput, lrc: &PlannedLrc) -> Result<(), LyricsFailure> {
        let fail = |error: Error| LyricsFailure {
            code: error.code(),
            message: error.to_string(),
        };
        let write = |state: OperationState| -> Result<(), LyricsFailure> {
            self.deps
                .journal
                .upsert_item(planned.operation, journal_lrc_item(state, planned, lrc))
                .map_err(fail)
        };
        write(OperationState::CopyPending)?;
        write(OperationState::CopyApplied)?;
        write(OperationState::ValidatePending)?;
        write(OperationState::Validated)?;
        write(OperationState::PublishPending)?;
        self.deps
            .fs
            .publish(planned.root, &lrc.staged, &lrc.target)
            .map_err(fail)?;
        let meta = self
            .deps
            .fs
            .file_meta(planned.root, &lrc.target)
            .map_err(fail)?;
        if meta.size != lrc.size {
            return Err(LyricsFailure {
                code: "corrupt_media",
                message: "published sidecar size differs from the staged content".to_owned(),
            });
        }
        let published_hash = self
            .deps
            .hasher
            .hash(planned.root, &lrc.target)
            .map_err(fail)?;
        if published_hash != lrc.hash {
            return Err(LyricsFailure {
                code: "corrupt_media",
                message: "published sidecar hash differs from the staged content".to_owned(),
            });
        }
        write(OperationState::PublishApplied)?;
        write(OperationState::Completed)?;
        Ok(())
    }

    /// Commit the song record and the journal's `DatabaseCommitted` state in
    /// one transaction, then finalize the operation and release the claim.
    /// `lrc_published` tells the commit whether the song's sidecar candidate
    /// may be wired from the published `.lrc` (a failed sidecar import leaves
    /// the song's sidecar source cleared — no fake lyrics).
    fn commit(
        &self,
        planned: &PlannedInput,
        parsed: &ParsedOutcome,
        lrc_published: bool,
    ) -> Result<(), Error> {
        let entity = song_from_parsed(
            planned.reserved,
            planned.root,
            &parsed.file,
            planned.added_at,
        );
        let entity_for_record = entity.clone();
        let embedded = parsed
            .embedded_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
        let sidecar = (lrc_published)
            .then(|| parsed.sidecar_lyrics.clone())
            .flatten()
            .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
        let cover = parsed.cover.clone();
        let resources = crate::application::scan_resources::verified_song_resources(
            self.deps,
            planned.root,
            parsed,
            lrc_published,
        )?;
        // Built before the transaction body: the closure owns the item.
        let committed = journal_item(OperationState::DatabaseCommitted, planned);
        let song_id = planned.reserved;
        let operation = planned.operation;
        crate::application::scan::durable::commit_song(
            self.deps,
            &entity_for_record,
            operation,
            crate::domain::library::durability::ChangeOrigin::Import,
            vec![crate::domain::library::durability::DurableResource {
                relative_path: planned.target.display().to_owned(),
                content_hash: planned.hash.clone(),
                byte_length: planned.size,
            }],
            || {
                self.deps
                    .uow
                    .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                        tx.set_song_file_resources(&entity, &resources)?;
                        tx.upsert_song(&entity)?;
                        match embedded {
                            Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                            None => tx.clear_lyrics_candidate(song_id, LyricsSource::Embedded)?,
                        }
                        match sidecar {
                            Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                            None => tx.clear_lyrics_candidate(song_id, LyricsSource::Sidecar)?,
                        }
                        if let Some(cover) = cover {
                            tx.attach_cover(song_id, &cover)?;
                        }
                        // The record and the journal's DatabaseCommitted state land in
                        // one transaction: a song is never visible without its journal.
                        tx.upsert_operation_item(operation, committed)?;
                        Ok(())
                    }))
            },
        )?;
        // Terminal state releases the target claim (port contract) so retries
        // and other operations can claim the same path again.
        self.deps.journal.upsert_item(
            planned.operation,
            journal_item(OperationState::Completed, planned),
        )?;
        self.deps.journal.release_claims(planned.operation)?;
        Ok(())
    }
}
