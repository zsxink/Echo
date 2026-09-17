use super::{report::*, *};

impl<'a> PlanImport<'a> {
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
        for song in self.deps.songs.all_in_root(root)? {
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
            .unwrap_or_else(|error| {
                tracing::warn!(
                    operation = %planned.operation,
                    %error,
                    "pre-commit dedup re-check could not read the library; continuing"
                );
                None
            })
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
        match parse_single_file(self.deps, planned.root, &planned.target) {
            FileOutcome::Parsed(parsed) => {
                self.commit(planned, &parsed, lrc_published)
                    .map_err(ExecuteError::PostPublish)?;
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
        let entity = song_from_parsed(planned.reserved, planned.root, &parsed.file);
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
        // Built before the transaction body: the closure owns the item.
        let committed = journal_item(OperationState::DatabaseCommitted, planned);
        let song_id = planned.reserved;
        let operation = planned.operation;
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
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
            }))?;
        // Materialize the portable song record once the row is durably
        // committed (design §2: the record is the durable form of the outbox
        // payload, sharing the exact revision/HLC the row holds). The gate
        // makes the "控制面不可写" scenario an explicit refusal of the import;
        // recovery re-drives idempotently if we crash here.
        crate::application::portable_materialize::ensure_control_plane_writable(
            self.deps.control.as_ref(),
            planned.root,
        )?;
        let device = self.deps.device_id.current_device_id();
        let (revision, hlc) = crate::application::portable_materialize::committed_version(
            self.deps.sync.as_ref(),
            "song",
            &planned.reserved.to_string(),
            device,
        )?;
        let record = crate::application::portable_materialize::song_record(
            device,
            hlc,
            revision,
            &entity_for_record,
        )?;
        self.deps
            .control
            .write_record(planned.root, &PortableRecord::Song(record))?;
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
