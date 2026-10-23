//! HostCommitLedgerPort impl — confirmation staging, commit pages, outcome records.

#[allow(unused_imports, clippy::wildcard_imports)]
use super::*;

#[allow(clippy::wildcard_imports)]
use super::host_ledger_apply::*;

impl HostCommitLedgerPort for SqliteDatabase {
    fn stage_confirmation(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<StagedConfirmation, Error> {
        self.stage_confirmation_at(binding, candidate, None)
    }

    /// Confirm a mutation using the edit time frozen on the sender's queue.
    ///
    /// The wire timestamp is the arbitration input: every device edits the same
    /// object, so the newest edit has to win. Falling back to the payload's
    /// legacy HLC only serves records written by an earlier release.
    #[allow(clippy::too_many_lines)]
    fn stage_confirmation_at(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
        changed_at_ms: Option<u64>,
    ) -> Result<StagedConfirmation, Error> {
        validate_candidate_payload(candidate)?;
        let candidate = candidate.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let configured: Option<(String, String, i64, String, Option<String>)> = tx.query_row(
                "SELECT library_id, authority_id, epoch, role, local_root_uuid FROM sync_binding WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(storage)?;
            let expected = (
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).map_err(|_| Error::validation(crate::error::Subject::Other, "epoch", "out of range"))?,
            );
            if configured.as_ref().is_none_or(|row| row.3 != "host" || row.4.is_none() || (row.0.as_str(), row.1.as_str(), row.2) != (expected.0.as_str(), expected.1.as_str(), expected.2)) {
                return Err(Error::conflict("host sync binding does not match this library"));
            }
            let payload = serde_json::to_vec(&candidate).map_err(storage)?;
            let payload_digest = blake3::hash(&payload).to_hex().to_string();
            let origin = candidate.origin_device_id.as_uuid().to_string();

            ensure_origin_authorized(&tx, &origin, &expected.0)?;

            if let Some(staged) = prior_confirmation(&tx, &origin, &candidate, &payload_digest)? {
                let root = super::super::durability::active_root(&tx)?;
                let operation = super::super::durability::identity("host-confirmation", &candidate.mutation_id.as_uuid().to_string());
                super::super::durability::commit_projection(control,tx,root,crate::domain::library::durability::ChangeOrigin::HostConfirmation,operation)?;
                return Ok(staged);
            }
            let candidate = normalize_member_alias_object(&tx, candidate)?;
            let submitted_resources = candidate.resource_refs.clone();
            let (candidate, song_alias) = super::super::song_aliases::normalize(&tx, candidate)?;


            // The independent checkpoint is published outside this transaction.
            // Admit only one unpublished commit so a later request cannot advance
            // the durable checkpoint past its predecessor after a crash/failure.
            // Exact inbox retries above remain available to finish publication.
            ensure_no_pending_predecessor(&tx, None)?;

            let (kind, object_uuid) = object_identity(candidate.object);
            let (already_deleted, current, current_changed_at_ms) =
                load_confirmed_object(&tx, &candidate)?;
            let current = exclude_final_playlist_members(&tx, candidate.object, current)?;
            let cursor = load_origin_cursor(&tx, candidate.origin_device_id)?;
            let cursor = recover_adjacent_terminal_predecessor(
                &tx,
                &origin,
                &candidate,
                cursor,
            )?;
            let next: i64 = tx.query_row("SELECT COALESCE(MAX(commit_seq), 0) + 1 FROM sync_commits", [], |row| row.get(0)).map_err(storage)?;
            let sequence = CommitSeq::from_u64(u64::try_from(next).map_err(storage)?);
            let canonical_candidate = exclude_historical_member_aliases(&tx, &candidate)?;
            // Arbitration input: the time frozen when the edit was queued. A
            // payload-embedded HLC only exists on records written by an earlier
            // release, so it is a fallback, never the primary source.
            let candidate_changed_at_ms =
                changed_at_ms.unwrap_or_else(|| payload_changed_at_ms(&candidate.payload).unwrap_or(0));
            let decision = confirm_candidate_at(
                current.as_ref(),
                cursor,
                &canonical_candidate,
                sequence,
                candidate_changed_at_ms,
                current_changed_at_ms,
            );

            if let Err(rejection) = decision {
                let staged = StagedConfirmation { mutation_id: candidate.mutation_id, outcome_json: rejection_outcome_json(rejection, cursor), checkpoint: None };
                // A permanent rejection still consumes a correctly chained
                // origin event. The replica advances its local cursor when it
                // records this outcome; keep the host cursor in lockstep so
                // later events do not remain blocked behind the rejected ID.
                if terminal_rejection_consumes_origin(rejection) {
                    advance_origin_cursor_for_terminal_rejection(
                        &tx, &origin, &candidate, cursor,
                    )?;
                }
                record_inbox_outcome(&tx, &origin, &candidate, &payload_digest, &staged, None)?;
                let root = super::super::durability::active_root(&tx)?;
                let operation = super::super::durability::identity("host-confirmation", &candidate.mutation_id.as_uuid().to_string());
                super::super::durability::commit_projection(control,tx,root,crate::domain::library::durability::ChangeOrigin::HostConfirmation,operation)?;
                return Ok(staged);
            }
            let confirmed = decision.map_err(|error| Error::conflict(format!("mutation rejected: {error:?}")))?;
            if already_deleted {
                let staged =
                    stage_already_deleted(&tx, &origin, &candidate, &payload_digest, &confirmed)?;
                let root = super::super::durability::active_root(&tx)?;
                let operation = super::super::durability::identity("host-confirmation", &candidate.mutation_id.as_uuid().to_string());
                super::super::durability::commit_projection(control,tx,root,crate::domain::library::durability::ChangeOrigin::HostConfirmation,operation)?;
                return Ok(staged);
            }
            let is_tombstone = candidate.payload == serde_json::json!({"tombstone": true});
            let previous_digest: Vec<u8> = tx.query_row("SELECT commit_digest FROM sync_commits ORDER BY commit_seq DESC LIMIT 1", [], |row| row.get(0)).optional().map_err(storage)?.unwrap_or_else(|| vec![0; 32]);
            if previous_digest.len() != 32 { return Err(Error::InvariantViolation { why: "invalid previous LAN commit digest".to_owned() }); }
            let previous_digest: [u8; 32] = previous_digest.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid previous LAN commit digest".to_owned() })?;
            let (canonical_json, commit_digest) = encode_commit(
                binding,
                &candidate,
                &confirmed.object.payload,
                sequence,
                previous_digest,
                &confirmed.aliases,
                song_alias.as_ref(),
            )?;
            tx.execute("INSERT INTO sync_commits(commit_seq, library_id, authority_id, epoch, origin_device_id, mutation_id, canonical_json, previous_digest, commit_digest, publication_state, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10)", params![next, expected.0, expected.1, expected.2, origin, candidate.mutation_id.as_uuid().to_string(), canonical_json, previous_digest, commit_digest, now_ms()]).map_err(storage)?;
            super::super::manifests::replace(&tx, kind, &object_uuid, &candidate.resource_refs)?;
            if let Some((alias, canonical)) = &song_alias {
                super::super::song_aliases::persist(&tx, alias, canonical, next)?;
                // Differing sidecars/covers remain durable held evidence under
                // the original alias, even when the canonical song keeps its
                // first-confirmed resource set.
                super::super::manifests::replace(&tx, "song", alias, &submitted_resources)?;
            }
            persist_confirmed_object(&tx, &candidate, &confirmed.object, next, is_tombstone, configured.as_ref().and_then(|row| row.4.as_deref()), confirmed.changed_at_ms)?;
            persist_member_aliases(&tx, &confirmed.aliases, next)?;
            if let Some((alias, canonical)) = &song_alias { super::super::song_aliases::project(&tx, alias, canonical)?; }
            tx.execute("INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id", params![origin, i64::try_from(confirmed.origin_cursor.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()]).map_err(storage)?;
            if let Some(conflict) = confirmed.conflict {
                tx.execute("INSERT INTO sync_conflicts(conflict_id, object_type, object_uuid, replaced_version, accepted_version, replaced_payload_json, accepted_payload_json, resolved_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)", params![super::super::durability::identity("conflict", &candidate.mutation_id.as_uuid().to_string()).to_string(), kind, object_uuid, i64::try_from(conflict.replaced_version.commit_seq().as_u64()).map_err(storage)?, next, serde_json::to_string(&conflict.replaced_payload).map_err(storage)?, serde_json::to_string(&conflict.accepted_payload).map_err(storage)?]).map_err(storage)?;
            }
            let checkpoint = CommitCheckpoint { binding, sequence, digest: commit_digest.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid LAN commit digest length".to_owned() })? };
            let staged = StagedConfirmation { mutation_id: candidate.mutation_id, outcome_json: "{\"accepted\":true}".to_owned(), checkpoint: Some(checkpoint) };
            record_inbox_outcome(&tx, &origin, &candidate, &payload_digest, &staged, Some(next))?;
            let root = super::super::durability::active_root(&tx)?;
            let operation = super::super::durability::identity("host-confirmation", &candidate.mutation_id.as_uuid().to_string());
            super::super::durability::commit_projection(control,tx,root,crate::domain::library::durability::ChangeOrigin::HostConfirmation,operation)?;
            Ok(staged)
        })
    }

    fn mark_published(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
        let control = self.library_control();
        self.writer.run_with_sync_wake(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let sequence = i64::try_from(checkpoint.sequence.as_u64()).map_err(storage)?;
            let library = checkpoint.binding.library_id.as_uuid().to_string();
            let authority = checkpoint.binding.authority_id.as_uuid().to_string();
            let epoch = i64::try_from(checkpoint.binding.epoch.as_u64()).map_err(storage)?;
            let publication: Option<(Vec<u8>, String, String)> = tx.query_row(
                "SELECT previous_digest, publication_state, origin_device_id FROM sync_commits WHERE commit_seq=?1 AND commit_digest=?2 AND library_id=?3 AND authority_id=?4 AND epoch=?5",
                params![sequence, checkpoint.digest.as_slice(), library, authority, epoch],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(storage)?;
            let Some((previous_digest, publication_state, origin_device)) = publication else {
                return Err(Error::conflict("publication checkpoint does not match a staged commit"));
            };
            // Validate and update in the same writer transaction, including on
            // exact retries. Neither a missing nor an unpublished predecessor
            // may be bypassed, even when repairing an older ledger.
            let (predecessors, invalid): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN publication_state!='published' OR library_id!=?2 OR authority_id!=?3 OR epoch!=?4 THEN 1 ELSE 0 END),0) FROM sync_commits WHERE commit_seq<?1",
                params![sequence, library, authority, epoch],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(storage)?;
            if predecessors != sequence - 1 || invalid != 0 {
                return Err(Error::conflict("publication requires a continuous published predecessor prefix"));
            }
            let expected_previous: Vec<u8> = if sequence == 1 {
                vec![0; 32]
            } else {
                tx.query_row("SELECT commit_digest FROM sync_commits WHERE commit_seq=?1", params![sequence - 1], |row| row.get(0)).map_err(storage)?
            };
            if previous_digest != expected_previous {
                return Err(Error::conflict("publication predecessor digest mismatch"));
            }
            let local_device: String = tx.query_row(
                "SELECT value FROM device_state WHERE key='device_id'", [], |row| row.get(0),
            ).map_err(storage)?;
            let sync_work = publication_state != "published" && origin_device != local_device;
            tx.execute("UPDATE sync_commits SET publication_state='published' WHERE commit_seq=?1 AND publication_state!='published'", params![sequence]).map_err(storage)?;
            let root = super::super::durability::active_root(&tx)?;
            let operation = super::super::durability::identity("host-published",&format!("{}:{}",checkpoint.binding.library_id.as_uuid(),checkpoint.sequence.as_u64()));
            super::super::durability::commit_projection(control,tx,root,crate::domain::library::durability::ChangeOrigin::HostConfirmation,operation)?;
            Ok(((), sync_work))
        })
    }

    fn pending_publications(&self, limit: usize) -> Result<Vec<StagedConfirmation>, Error> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.writer.run(move |connection| {
            let mut statement = connection.prepare("SELECT i.outcome_json FROM sync_inbox i JOIN sync_commits c ON c.mutation_id=i.mutation_id AND c.origin_device_id=i.origin_device_id WHERE c.publication_state='pending' ORDER BY c.commit_seq LIMIT ?1").map_err(storage)?;
            let rows = statement.query_map(params![limit], |row| row.get::<_, String>(0)).map_err(storage)?;
            rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage)).collect()
        })
    }

    fn published_commits_after(
        &self,
        binding: SyncBinding,
        after: CommitSeq,
        expected_digest: [u8; 32],
        limit: usize,
    ) -> Result<Vec<PublishedCommit>, Error> {
        if limit == 0 || limit > 512 {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "commit page size",
                "must be between 1 and 512",
            ));
        }
        // A held long-poll re-reads this page every 500ms per peer. It must not
        // queue behind the single writer actor: a confirmation transaction
        // (fsync + checkpoint advance) would otherwise delay every watching
        // peer's recheck, and the rechecks themselves would interleave with the
        // publisher's own writes. The read is satisfied from WAL by a pooled
        // read-only connection, so it neither takes the writer lock nor blocks
        // the actor.
        self.with_reader(move |connection| {
            let tx = connection.unchecked_transaction().map_err(storage)?;
            // Genesis has no log row from which to infer its binding. Validate
            // the configured host even for an empty log or an empty response.
            let matching_host: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND local_root_uuid IS NOT NULL AND library_id=?1 AND authority_id=?2 AND epoch=?3)",
                params![binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
                |row| row.get(0),
            ).map_err(storage)?;
            if !matching_host {
                return Err(Error::conflict("host sync binding does not match this library"));
            }
            let after_i64 = i64::try_from(after.as_u64()).map_err(storage)?;
            let anchor: Option<Vec<u8>> = if after_i64 == 0 { None } else {
                tx.query_row("SELECT commit_digest FROM sync_commits WHERE commit_seq=?1 AND publication_state='published' AND library_id=?2 AND authority_id=?3 AND epoch=?4", params![after_i64, binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?], |row| row.get(0)).optional().map_err(storage)?
            };
            let actual_anchor = match anchor {
                Some(bytes) => bytes.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid published anchor digest".to_owned() })?,
                None if after_i64 == 0 => [0u8; 32],
                None => return Err(Error::conflict("requested commit anchor is unavailable")),
            };
            if actual_anchor != expected_digest {
                return Err(Error::conflict("commit anchor digest mismatch"));
            }
            let mut statement = tx.prepare("SELECT commit_seq, previous_digest, commit_digest, canonical_json FROM sync_commits WHERE commit_seq>?1 AND publication_state='published' AND library_id=?2 AND authority_id=?3 AND epoch=?4 ORDER BY commit_seq LIMIT ?5").map_err(storage)?;
            let rows = statement.query_map(params![after_i64, binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?, i64::try_from(limit).map_err(storage)?], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, String>(3)?))).map_err(storage)?;
            let mut commits = Vec::new();
            let mut expected_sequence = after_i64;
            let mut previous = expected_digest;
            for row in rows {
                let (sequence, previous_bytes, digest_bytes, canonical_json) = row.map_err(storage)?;
                expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| Error::InvariantViolation { why: "published sequence overflow".to_owned() })?;
                let previous_digest: [u8; 32] = previous_bytes.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid previous digest in commit log".to_owned() })?;
                let digest: [u8; 32] = digest_bytes.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid digest in commit log".to_owned() })?;
                if sequence != expected_sequence || previous_digest != previous {
                    return Err(Error::conflict("published commit log contains a sequence gap or fork"));
                }
                let body = canonical_json.as_bytes();
                let body_len = u32::try_from(body.len()).map_err(storage)?;
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"echo.lan.commit.v1\0");
                hasher.update(&1u16.to_be_bytes());
                hasher.update(&u64::try_from(sequence).map_err(storage)?.to_be_bytes());
                hasher.update(&previous_digest);
                hasher.update(&body_len.to_be_bytes());
                hasher.update(body);
                if hasher.finalize().as_bytes() != &digest {
                    return Err(Error::conflict("published commit digest does not verify"));
                }
                commits.push(PublishedCommit { sequence: CommitSeq::from_u64(u64::try_from(sequence).map_err(storage)?), previous_digest, digest, canonical_json });
                previous = digest;
            }
            drop(statement);
            tx.commit().map_err(storage)?;
            Ok(commits)
        })
    }
}
