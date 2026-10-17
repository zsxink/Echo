//! Durable host confirmation ledger backed by the library SQLite writer.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::application::ports::{
    HostCommitLedgerPort, HostPeerAuthorizationPort, PublishedCommit, StagedConfirmation,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::sync::{
    confirm_candidate, CommitCheckpoint, CommitSeq, ConfirmationError, ConfirmedObject, DeviceId,
    MutationCandidate, MutationId, ObjectKey, ObjectVersion, OriginCursor, SyncBinding,
};
use crate::error::Error;

use super::support::{now_ms, storage};
use super::SqliteDatabase;

impl SqliteDatabase {
    /// Detach one local library from LAN sync while preserving its library rows.
    /// Protocol state for the selected and archived copies of this root is
    /// removed so selecting the root later cannot silently restore the old peer
    /// or host binding.
    pub fn detach_lan_sync_context(&self, local_root: LibraryRootId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let root = local_root.to_string();
            super::sync_context::select(&tx, &root)?;
            for table in [
                "sync_snapshot_pages",
                "sync_snapshot_resource_refs",
                "sync_snapshot_records",
                "sync_snapshot_sessions",
                "sync_replica_snapshot_pages",
                "sync_replica_snapshot_records",
                "sync_replica_snapshot_sessions",
                "sync_host_baselines",
                "sync_transfers",
                "sync_peer_cursors",
                "sync_conflicts",
                "sync_member_aliases",
                "sync_song_aliases",
                "sync_object_versions",
                "sync_commits",
                "sync_inbox",
                "sync_origin_cursors",
                "sync_peers",
                "sync_binding",
            ] {
                tx.execute_batch(&format!("DELETE FROM {table};"))
                    .map_err(storage)?;
                tx.execute(
                    &format!("DELETE FROM archived_{table} WHERE context_root=?1"),
                    params![root],
                )
                .map_err(storage)?;
            }
            for table in [
                "sync_remote_resource_locations",
                "sync_remote_publication_claims",
                "sync_remote_publications",
                "sync_mutations",
                "sync_outbox",
                "sync_local_origin_cursors",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE local_root_uuid=?1"),
                    params![root],
                )
                .map_err(storage)?;
            }
            tx.execute(
                "DELETE FROM sync_state WHERE key LIKE ?1",
                params![format!("%{root}%")],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)
        })
    }

    /// Clear LAN sync lineage while preserving library rows and media metadata.
    /// Intended only for an explicit local rebuild after secure credentials are
    /// lost; the next host start creates a new authority and initial baseline.
    pub fn reset_lan_sync_for_rebuild(&self) -> Result<(), Error> {
        self.writer.run(|connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            for table in [
                "sync_snapshot_pages",
                "sync_snapshot_resource_refs",
                "sync_snapshot_records",
                "sync_snapshot_sessions",
                "sync_replica_snapshot_pages",
                "sync_replica_snapshot_records",
                "sync_replica_snapshot_sessions",
                "sync_host_baselines",
                "sync_transfers",
                "sync_peer_cursors",
                "sync_conflicts",
                "sync_member_aliases",
                "sync_song_aliases",
                "sync_object_versions",
                "sync_commits",
                "sync_inbox",
                "sync_origin_cursors",
                "sync_peers",
                "sync_binding",
            ] {
                tx.execute_batch(&format!("DELETE FROM {table};"))
                    .map_err(storage)?;
            }
            for table in ["sync_remote_resource_locations", "sync_remote_publication_claims", "sync_remote_publications"] {
                tx.execute(&format!("DELETE FROM {table} WHERE local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)"), []).map_err(storage)?;
            }
            for table in ["sync_mutations", "sync_outbox", "sync_local_origin_cursors"] {
                tx.execute(&format!("DELETE FROM {table} WHERE local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)"), []).map_err(storage)?;
            }
            tx.commit().map_err(storage)
        })
    }

    /// Establish the library identity and authority before enabling LAN sync.
    /// Rebinding an existing database to a different identity is rejected;
    /// explicit recovery must create a new epoch through a separate workflow.
    pub fn bind_sync_library(
        &self,
        binding: SyncBinding,
        host: bool,
        local_root: LibraryRootId,
    ) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let local_root_uuid = local_root.to_string();
            super::sync_context::select(&tx, &local_root_uuid)?;
            let root_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)",
                params![local_root_uuid],
                |row| row.get(0),
            ).map_err(storage)?;
            if !root_exists {
                return Err(Error::conflict("sync binding requires an existing local library root"));
            }
            tx.execute("INSERT OR IGNORE INTO sync_root_contexts(local_root_uuid) VALUES(?1)", params![local_root_uuid]).map_err(storage)?;
            let current: Option<(String, String, i64, String, Option<String>)> = tx.query_row(
                "SELECT library_id, authority_id, epoch, role, local_root_uuid FROM sync_binding WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(storage)?;
            let expected = (
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).map_err(|_| Error::validation(crate::error::Subject::Other, "epoch", "out of range"))?,
                if host { "host" } else { "replica" },
            );
            if let Some(existing) = current {
                let same_binding = (existing.0.as_str(), existing.1.as_str(), existing.2, existing.3.as_str())
                    == (expected.0.as_str(), expected.1.as_str(), expected.2, expected.3);
                if !same_binding || existing.4.as_deref().is_some_and(|root| root != local_root_uuid) {
                    return Err(Error::conflict("sync library binding already exists"));
                }
                if existing.4.is_none() {
                    tx.execute(
                        "UPDATE sync_binding SET local_root_uuid=?1, updated_at=?2 WHERE singleton=1 AND local_root_uuid IS NULL",
                        params![local_root_uuid, now_ms()],
                    ).map_err(storage)?;
                    tx.commit().map_err(storage)?;
                }
                return Ok(());
            }
            tx.execute(
                "INSERT INTO sync_binding(singleton, library_id, authority_id, epoch, role, protocol_version, updated_at, local_root_uuid) VALUES(1, ?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![expected.0, expected.1, expected.2, expected.3, now_ms(), local_root_uuid],
            ).map_err(storage)?;
            tx.commit().map_err(storage)
        })
    }

    /// Atomically bind an empty local target as a replica. Host snapshots may
    /// only be applied after this empty-state check has committed; an existing
    /// replica retry is idempotent only for the exact same binding and root.
    pub fn bind_empty_sync_replica(
        &self,
        binding: SyncBinding,
        local_root: LibraryRootId,
    ) -> Result<(), Error> {
        self.bind_replica_for_enrollment(binding, local_root, false)
    }

    /// Enroll the selected local library while preserving its local-first state.
    /// Existing rows remain intact as the complete host snapshot is merged.
    pub fn bind_sync_replica_for_enrollment(
        &self,
        binding: SyncBinding,
        local_root: LibraryRootId,
    ) -> Result<(), Error> {
        self.bind_replica_for_enrollment(binding, local_root, true)
    }

    fn bind_replica_for_enrollment(
        &self,
        binding: SyncBinding,
        local_root: LibraryRootId,
        allow_nonempty_catalog: bool,
    ) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let root = local_root.to_string();
            super::sync_context::select(&tx, &root)?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)",
                params![root],
                |row| row.get(0),
            ).map_err(storage)?;
            if !exists {
                return Err(Error::conflict("replica enrollment requires an existing local root"));
            }
            tx.execute("INSERT OR IGNORE INTO sync_root_contexts(local_root_uuid) VALUES(?1)", params![root]).map_err(storage)?;
            let current: Option<(String, String, i64, String, Option<String>)> = tx.query_row(
                "SELECT library_id, authority_id, epoch, role, local_root_uuid FROM sync_binding WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(storage)?;
            if let Some((library, authority, epoch, role, bound_root)) = current {
                let exact = library == binding.library_id.as_uuid().to_string()
                    && authority == binding.authority_id.as_uuid().to_string()
                    && epoch == i64::try_from(binding.epoch.as_u64()).map_err(storage)?
                    && role == "replica"
                    && bound_root.as_deref() == Some(root.as_str());
                if exact {
                    tx.commit().map_err(storage)?;
                    return Ok(());
                }
                return Err(Error::conflict("library is already bound to another sync identity"));
            }
            let nonempty: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE library_root_uuid=?1) OR EXISTS(SELECT 1 FROM playlists WHERE library_root_uuid=?1) OR EXISTS(SELECT 1 FROM sync_mutations WHERE status='pending' AND local_root_uuid=?1)",
                params![root],
                |row| row.get(0),
            ).map_err(storage)?;
            if nonempty && !allow_nonempty_catalog {
                return Err(Error::conflict("replica enrollment target is not empty"));
            }
            tx.execute(
                "INSERT INTO sync_binding(singleton, library_id, authority_id, epoch, role, protocol_version, updated_at, local_root_uuid) VALUES(1, ?1, ?2, ?3, 'replica', 1, ?4, ?5)",
                params![binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?, now_ms(), root],
            ).map_err(crate::infrastructure::sqlite::support::map_constraint)?;
            if allow_nonempty_catalog {
                tx.execute(
                    "INSERT OR REPLACE INTO sync_state(key,value) VALUES(?1,?2)",
                    params![format!("replica_enrollment_replace:{root}"), serde_json::to_string(&binding).map_err(storage)?],
                ).map_err(storage)?;
            }
            tx.commit().map_err(storage)
        })
    }

    /// Register an authenticated peer. `credential_handle` is an opaque
    /// platform secure-store reference, never the credential itself.
    pub fn authorize_sync_peer(
        &self,
        binding: SyncBinding,
        device: DeviceId,
        display_name: String,
        credential_handle: String,
    ) -> Result<(), Error> {
        if display_name.trim().is_empty()
            || display_name.len() > 128
            || display_name.chars().any(char::is_control)
        {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "device display name",
                "invalid display name",
            ));
        }
        if credential_handle.is_empty()
            || credential_handle.len() > 512
            || credential_handle.chars().any(char::is_control)
        {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "credential handle",
                "invalid secure-store reference",
            ));
        }
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let authorized_host: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND library_id=?1 AND authority_id=?2 AND epoch=?3)",
                params![binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
                |row| row.get(0),
            ).map_err(storage)?;
            if !authorized_host { return Err(Error::conflict("peer authorization requires the matching host binding")); }
            tx.execute(
                "INSERT INTO sync_peers(device_id, library_id, display_name, credential_handle, authorized_at, revoked_at) VALUES(?1, ?2, ?3, ?4, ?5, NULL)",
                params![device.as_uuid().to_string(), binding.library_id.as_uuid().to_string(), display_name, credential_handle, now_ms()],
            ).map_err(crate::infrastructure::sqlite::support::map_constraint)?;
            tx.commit().map_err(storage)
        })
    }

    /// Authorize a newly paired device or replace only its revoked generation.
    /// An active credential is never overwritten by a stale pairing request.
    pub fn reauthorize_sync_peer(
        &self,
        binding: SyncBinding,
        device: DeviceId,
        display_name: String,
        credential_handle: String,
    ) -> Result<(), Error> {
        if display_name.trim().is_empty()
            || display_name.len() > 128
            || display_name.chars().any(char::is_control)
        {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "device display name",
                "invalid display name",
            ));
        }
        if credential_handle.is_empty()
            || credential_handle.len() > 512
            || credential_handle.chars().any(char::is_control)
        {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "credential handle",
                "invalid secure-store reference",
            ));
        }
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let binding_matches: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND library_id=?1 AND authority_id=?2 AND epoch=?3)",
                params![binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
                |row| row.get(0),
            ).map_err(storage)?;
            if !binding_matches {
                return Err(Error::conflict("peer authorization requires the matching host binding"));
            }
            let changed = tx.execute(
                "INSERT INTO sync_peers(device_id, library_id, display_name, credential_handle, authorized_at, revoked_at) VALUES(?1, ?2, ?3, ?4, ?5, NULL) ON CONFLICT(device_id) DO UPDATE SET display_name=excluded.display_name, credential_handle=excluded.credential_handle, authorized_at=excluded.authorized_at, revoked_at=NULL WHERE sync_peers.library_id=excluded.library_id AND sync_peers.revoked_at IS NOT NULL",
                params![device.as_uuid().to_string(), binding.library_id.as_uuid().to_string(), display_name, credential_handle, now_ms()],
            ).map_err(crate::infrastructure::sqlite::support::map_constraint)?;
            if changed != 1 {
                return Err(Error::conflict("peer is already authorized"));
            }
            tx.commit().map_err(storage)
        })
    }

    /// Revoke one peer without changing its local library or deleting history.
    pub fn revoke_sync_peer(&self, device: DeviceId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE sync_peers SET revoked_at=?2 WHERE device_id=?1 AND revoked_at IS NULL",
                    params![device.as_uuid().to_string(), now_ms()],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(Error::conflict("sync peer is not currently authorized"))
            }
        })
    }

    /// Check that this exact credential generation is still authorized for the
    /// supplied host binding. Pairing claims must not load a later credential
    /// merely because it belongs to the same device id.
    pub fn sync_peer_has_credential(
        &self,
        binding: SyncBinding,
        device: DeviceId,
        credential_handle: String,
    ) -> Result<bool, Error> {
        self.writer.run(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_peers p JOIN sync_binding b ON b.singleton=1 WHERE p.device_id=?1 AND p.library_id=?2 AND p.credential_handle=?3 AND p.revoked_at IS NULL AND b.role='host' AND b.library_id=?2 AND b.authority_id=?4 AND b.epoch=?5)",
                    params![
                        device.as_uuid().to_string(),
                        binding.library_id.as_uuid().to_string(),
                        credential_handle,
                        binding.authority_id.as_uuid().to_string(),
                        i64::try_from(binding.epoch.as_u64()).map_err(storage)?,
                    ],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
    }

    /// Conditionally revoke only the credential generation created by one
    /// pairing request. A stale cleanup is a harmless no-op and cannot revoke
    /// a subsequently approved generation for the same device.
    pub fn revoke_sync_peer_credential(
        &self,
        binding: SyncBinding,
        device: DeviceId,
        credential_handle: String,
    ) -> Result<bool, Error> {
        self.writer.run(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE sync_peers SET revoked_at=?5 WHERE device_id=?1 AND library_id=?2 AND credential_handle=?3 AND revoked_at IS NULL AND EXISTS(SELECT 1 FROM sync_binding b WHERE b.singleton=1 AND b.role='host' AND b.library_id=?2 AND b.authority_id=?4 AND b.epoch=?6)",
                    params![
                        device.as_uuid().to_string(),
                        binding.library_id.as_uuid().to_string(),
                        credential_handle,
                        binding.authority_id.as_uuid().to_string(),
                        now_ms(),
                        i64::try_from(binding.epoch.as_u64()).map_err(storage)?,
                    ],
                )
                .map_err(storage)?;
            Ok(changed == 1)
        })
    }
}

impl HostCommitLedgerPort for SqliteDatabase {
    fn stage_confirmation(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<StagedConfirmation, Error> {
        super::sync_manifests::validate(&candidate.resource_refs)?;
        if candidate.payload == serde_json::json!({"tombstone": true})
            && !candidate.resource_refs.is_empty()
        {
            return Err(Error::conflict(
                "deleted objects cannot declare resource references",
            ));
        }
        let candidate = candidate.clone();
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
                tx.commit().map_err(storage)?;
                return Ok(staged);
            }
            let candidate = normalize_member_alias_object(&tx, candidate)?;
            let submitted_resources = candidate.resource_refs.clone();
            let (candidate, song_alias) = super::sync_song_aliases::normalize(&tx, candidate)?;


            // The independent checkpoint is published outside this transaction.
            // Admit only one unpublished commit so a later request cannot advance
            // the durable checkpoint past its predecessor after a crash/failure.
            // Exact inbox retries above remain available to finish publication.
            ensure_no_pending_predecessor(&tx, None)?;

            let (kind, object_uuid) = object_identity(candidate.object);
            let (current_is_tombstone, current) = load_confirmed_object(&tx, candidate.object)?;
            let current = exclude_final_playlist_members(&tx, candidate.object, current)?;
            let cursor = load_origin_cursor(&tx, candidate.origin_device_id)?;
            let next: i64 = tx.query_row("SELECT COALESCE(MAX(commit_seq), 0) + 1 FROM sync_commits", [], |row| row.get(0)).map_err(storage)?;
            let sequence = CommitSeq::from_u64(u64::try_from(next).map_err(storage)?);
            let canonical_candidate = exclude_historical_member_aliases(&tx, &candidate)?;
            let decision = confirm_candidate(current.as_ref(), cursor, &canonical_candidate, sequence);

            if let Err(rejection) = decision {
                let staged = StagedConfirmation { mutation_id: candidate.mutation_id, outcome_json: format!("{{\"accepted\":false,\"reason\":\"{rejection:?}\"}}"), checkpoint: None };
                // A permanent rejection still consumes a correctly chained
                // origin event. The replica advances its local cursor when it
                // records this outcome; keep the host cursor in lockstep so
                // later events do not remain blocked behind the rejected ID.
                if terminal_rejection_consumes_origin(rejection) {
                    advance_origin_cursor_for_terminal_rejection(
                        &tx, &origin, &candidate, cursor,
                    )?;
                }
                let serialized = serde_json::to_string(&staged).map_err(storage)?;
                tx.execute("INSERT INTO sync_inbox(origin_device_id, mutation_id, payload_digest, outcome_json, commit_seq, received_at) VALUES(?1, ?2, ?3, ?4, NULL, ?5) ON CONFLICT(origin_device_id, mutation_id) DO UPDATE SET outcome_json=excluded.outcome_json, commit_seq=NULL, received_at=excluded.received_at", params![origin, candidate.mutation_id.as_uuid().to_string(), payload_digest, serialized, now_ms()]).map_err(storage)?;
                tx.commit().map_err(storage)?;
                return Ok(staged);
            }
            let confirmed = decision.map_err(|error| Error::conflict(format!("mutation rejected: {error:?}")))?;
            if current_is_tombstone {
                // A final rejection consumes a correctly chained origin event.
                // Otherwise every later event from that device would wait
                // forever behind an object that can never be resurrected.
                let staged = StagedConfirmation {
                    mutation_id: candidate.mutation_id,
                    outcome_json: "{\"accepted\":false,\"reason\":\"object_already_deleted\"}".to_owned(),
                    checkpoint: None,
                };
                tx.execute(
                    "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
                    params![origin, i64::try_from(confirmed.origin_cursor.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()],
                ).map_err(storage)?;
                let serialized = serde_json::to_string(&staged).map_err(storage)?;
                tx.execute("INSERT INTO sync_inbox(origin_device_id, mutation_id, payload_digest, outcome_json, commit_seq, received_at) VALUES(?1, ?2, ?3, ?4, NULL, ?5) ON CONFLICT(origin_device_id, mutation_id) DO UPDATE SET outcome_json=excluded.outcome_json, commit_seq=NULL, received_at=excluded.received_at", params![origin, candidate.mutation_id.as_uuid().to_string(), payload_digest, serialized, now_ms()]).map_err(storage)?;
                tx.commit().map_err(storage)?;
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
            super::sync_manifests::replace(&tx, kind, &object_uuid, &candidate.resource_refs)?;
            if let Some((alias, canonical)) = &song_alias {
                super::sync_song_aliases::persist(&tx, alias, canonical, next)?;
                // Differing sidecars/covers remain durable held evidence under
                // the original alias, even when the canonical song keeps its
                // first-confirmed resource set.
                super::sync_manifests::replace(&tx, "song", alias, &submitted_resources)?;
            }
            persist_confirmed_object(&tx, &candidate, &confirmed.object, next, is_tombstone, configured.as_ref().and_then(|row| row.4.as_deref()))?;
            persist_member_aliases(&tx, &confirmed.aliases, next)?;
            if let Some((alias, canonical)) = &song_alias { super::sync_song_aliases::project(&tx, alias, canonical)?; }
            tx.execute("INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id", params![origin, i64::try_from(confirmed.origin_cursor.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()]).map_err(storage)?;
            if let Some(conflict) = confirmed.conflict {
                tx.execute("INSERT INTO sync_conflicts(conflict_id, object_type, object_uuid, replaced_version, accepted_version, replaced_payload_json, accepted_payload_json, resolved_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)", params![uuid::Uuid::new_v4().to_string(), kind, object_uuid, i64::try_from(conflict.replaced_version.commit_seq().as_u64()).map_err(storage)?, next, serde_json::to_string(&conflict.replaced_payload).map_err(storage)?, serde_json::to_string(&conflict.accepted_payload).map_err(storage)?]).map_err(storage)?;
            }
            let checkpoint = CommitCheckpoint { binding, sequence, digest: commit_digest.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid LAN commit digest length".to_owned() })? };
            let staged = StagedConfirmation { mutation_id: candidate.mutation_id, outcome_json: "{\"accepted\":true}".to_owned(), checkpoint: Some(checkpoint) };
            let serialized = serde_json::to_string(&staged).map_err(storage)?;
            tx.execute("INSERT INTO sync_inbox(origin_device_id, mutation_id, payload_digest, outcome_json, commit_seq, received_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(origin_device_id, mutation_id) DO UPDATE SET outcome_json=excluded.outcome_json, commit_seq=excluded.commit_seq, received_at=excluded.received_at", params![origin, candidate.mutation_id.as_uuid().to_string(), payload_digest, serialized, next, now_ms()]).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(staged)
        })
    }

    fn mark_published(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let sequence = i64::try_from(checkpoint.sequence.as_u64()).map_err(storage)?;
            let library = checkpoint.binding.library_id.as_uuid().to_string();
            let authority = checkpoint.binding.authority_id.as_uuid().to_string();
            let epoch = i64::try_from(checkpoint.binding.epoch.as_u64()).map_err(storage)?;
            let previous_digest: Option<Vec<u8>> = tx.query_row(
                "SELECT previous_digest FROM sync_commits WHERE commit_seq=?1 AND commit_digest=?2 AND library_id=?3 AND authority_id=?4 AND epoch=?5",
                params![sequence, checkpoint.digest.as_slice(), library, authority, epoch],
                |row| row.get(0),
            ).optional().map_err(storage)?;
            let Some(previous_digest) = previous_digest else {
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
            tx.execute("UPDATE sync_commits SET publication_state='published' WHERE commit_seq=?1", params![sequence]).map_err(storage)?;
            tx.commit().map_err(storage)
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
        self.writer.run(move |connection| {
            let tx = connection.transaction().map_err(storage)?;
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

impl HostPeerAuthorizationPort for SqliteDatabase {
    fn peer_is_authorized(
        &self,
        binding: SyncBinding,
        peer_device_id: DeviceId,
    ) -> Result<bool, Error> {
        self.with_reader(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_binding b JOIN sync_peers p ON p.library_id=b.library_id WHERE b.singleton=1 AND b.role='host' AND b.library_id=?1 AND b.authority_id=?2 AND b.epoch=?3 AND p.device_id=?4 AND p.revoked_at IS NULL)",
                    params![
                        binding.library_id.as_uuid().to_string(),
                        binding.authority_id.as_uuid().to_string(),
                        i64::try_from(binding.epoch.as_u64()).map_err(storage)?,
                        peer_device_id.as_uuid().to_string(),
                    ],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
    }
}

fn ensure_origin_authorized(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    library: &str,
) -> Result<(), Error> {
    let local_device: String = tx
        .query_row(
            "SELECT value FROM device_state WHERE key = 'device_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let authorized = local_device == origin || tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_peers WHERE device_id=?1 AND library_id=?2 AND revoked_at IS NULL)",
        params![origin, library],
        |row| row.get::<_, bool>(0),
    ).map_err(storage)?;
    if authorized {
        Ok(())
    } else {
        Err(Error::conflict(
            "origin device is not authorized for this library",
        ))
    }
}

fn persist_member_aliases(
    tx: &rusqlite::Transaction<'_>,
    aliases: &[crate::domain::sync::PlaylistMemberAlias],
    sequence: i64,
) -> Result<(), Error> {
    for alias in aliases {
        let playlist = alias.playlist_id.to_string();
        let alias_id = alias.alias_member_id.to_string();
        let canonical_id = alias.canonical_member_id.to_string();
        let prior: Option<String> = tx.query_row(
            "SELECT canonical_member_uuid FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2",
            params![playlist, alias_id],
            |row| row.get(0),
        ).optional().map_err(storage)?;
        if prior.as_deref().is_some_and(|value| value != canonical_id) {
            return Err(Error::conflict(
                "playlist member alias already has a different canonical identity",
            ));
        }
        let aliases_target_alias: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
            params![playlist, canonical_id],
            |row| row.get(0),
        ).map_err(storage)?;
        if aliases_target_alias {
            return Err(Error::conflict(
                "playlist member alias target is not canonical",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)",
            params![playlist, alias_id, canonical_id, sequence],
        ).map_err(storage)?;
    }
    Ok(())
}

fn prior_confirmation(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    candidate: &MutationCandidate,
    payload_digest: &str,
) -> Result<Option<StagedConfirmation>, Error> {
    let prior: Option<(String, String, Option<i64>)> = tx.query_row(
        "SELECT payload_digest, outcome_json, commit_seq FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
        params![origin, candidate.mutation_id.as_uuid().to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(storage)?;
    let Some((digest, outcome, commit_seq)) = prior else {
        return Ok(None);
    };
    if digest != payload_digest {
        return Err(Error::conflict(
            "mutation identity was reused with different content",
        ));
    }
    let staged: StagedConfirmation = serde_json::from_str(&outcome).map_err(storage)?;
    let retryable_predecessor_rejection =
        commit_seq.is_none() && staged.outcome_json.contains("OutOfOrderPredecessor");
    if retryable_predecessor_rejection {
        return Ok(None);
    }
    if commit_seq.is_none() && terminal_rejection_consumes_origin_json(&staged.outcome_json) {
        let candidate = normalize_member_alias_object(tx, candidate.clone())?;
        let cursor = load_origin_cursor(tx, candidate.origin_device_id)?;
        advance_origin_cursor_for_terminal_rejection(tx, origin, &candidate, cursor)?;
    }
    if let Some(sequence) = commit_seq {
        ensure_no_pending_predecessor(tx, Some(sequence))?;
    }
    Ok(Some(staged))
}

fn terminal_rejection_consumes_origin(rejection: ConfirmationError) -> bool {
    matches!(
        rejection,
        ConfirmationError::WrongObject
            | ConfirmationError::WrongOrigin
            | ConfirmationError::UnauthorizedPlayStatsBucket
    )
}

fn terminal_rejection_consumes_origin_json(outcome_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(outcome_json)
        .ok()
        .and_then(|value| value.get("reason")?.as_str().map(str::to_owned))
        .is_some_and(|reason| {
            matches!(
                reason.as_str(),
                "WrongObject" | "WrongOrigin" | "UnauthorizedPlayStatsBucket"
            )
        })
}

fn advance_origin_cursor_for_terminal_rejection(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    candidate: &MutationCandidate,
    cursor: Option<OriginCursor>,
) -> Result<(), Error> {
    let expected_seq = cursor.map_or(1, |value| value.origin_seq.saturating_add(1));
    let expected_previous = cursor.map(|value| value.last_mutation_id);
    if candidate.origin_seq == expected_seq && candidate.previous_mutation_id == expected_previous
    {
        tx.execute(
            "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
            params![origin, i64::try_from(candidate.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()],
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn ensure_no_pending_predecessor(
    tx: &rusqlite::Transaction<'_>,
    before_sequence: Option<i64>,
) -> Result<(), Error> {
    let pending: bool = before_sequence.map_or_else(
        || tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_commits WHERE publication_state='pending')",
            [],
            |row| row.get(0),
        ),
        |sequence| tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_commits WHERE publication_state='pending' AND commit_seq<?1)",
            params![sequence],
            |row| row.get(0),
        ),
    ).map_err(storage)?;
    if pending {
        Err(Error::unavailable(
            "sync publication",
            "publish the pending predecessor before confirming this mutation",
        ))
    } else {
        Ok(())
    }
}

fn object_identity(object: ObjectKey) -> (&'static str, String) {
    match object {
        ObjectKey::Song(id) => ("song", id.to_string()),
        ObjectKey::Playlist(id) => ("playlist", id.to_string()),
        ObjectKey::PlaylistItem(id) => ("playlist-item", id.to_string()),
        ObjectKey::LibraryRoot(id) => ("library-root", id.to_string()),
        ObjectKey::Favorite(id) => ("favorite", id.to_string()),
        ObjectKey::Override(id) => ("override", id.to_string()),
        ObjectKey::PlayStats(id) => ("play-stats", id.to_string()),
    }
}

fn persist_confirmed_object(
    tx: &rusqlite::Transaction<'_>,
    candidate: &MutationCandidate,
    object: &ConfirmedObject,
    next: i64,
    is_tombstone: bool,
    local_root_uuid: Option<&str>,
) -> Result<(), Error> {
    let origin = candidate.origin_device_id.as_uuid().to_string();
    let (kind, object_uuid) = object_identity(object.object);
    let version = i64::try_from(object.version.commit_seq().as_u64()).map_err(storage)?;
    tx.execute("INSERT INTO sync_object_versions(object_type, object_uuid, object_version, commit_seq, payload_json, is_tombstone) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(object_type, object_uuid) DO UPDATE SET object_version=excluded.object_version, commit_seq=excluded.commit_seq, payload_json=excluded.payload_json, is_tombstone=excluded.is_tombstone", params![kind, object_uuid, version, next, if is_tombstone { None } else { Some(serde_json::to_string(&object.payload).map_err(storage)?) }, i64::from(is_tombstone)]).map_err(storage)?;
    tx.execute(
                "UPDATE sync_mutations SET status='confirmed', outcome_json=?3 WHERE origin_device_id=?1 AND mutation_id=?2 AND status='pending'",
                params![origin, candidate.mutation_id.as_uuid().to_string(), "{\"accepted\":true}"],
            )
            .map_err(storage)?;
    if is_tombstone {
        super::sync_projection::project_confirmed_tombstone(tx, kind, &object_uuid)?;
    } else {
        super::sync_projection::project_confirmed_song_state(
            tx,
            kind,
            &object_uuid,
            &object.payload,
            local_root_uuid,
        )?;
    }
    Ok(())
}

fn encode_commit(
    binding: SyncBinding,
    candidate: &MutationCandidate,
    confirmed_payload: &serde_json::Value,
    sequence: CommitSeq,
    previous_digest: [u8; 32],
    aliases: &[crate::domain::sync::PlaylistMemberAlias],
    song_alias: Option<&(String, String)>,
) -> Result<(String, Vec<u8>), Error> {
    let mut confirmed_candidate = candidate.clone();
    // The submitted mutation remains immutable in the sender's outbox. The
    // host log records its deterministic canonical result for every replica.
    confirmed_candidate.payload = confirmed_payload.clone();
    let commit_body = WireCommitBody {
        protocol_version: 1,
        binding,
        commit_seq: sequence,
        previous_digest,
        mutation: WireMutationBody {
            protocol_version: 1,
            binding,
            origin_device_id: candidate.origin_device_id,
            mutation_id: candidate.mutation_id,
            origin_seq: candidate.origin_seq,
            previous_mutation_id: candidate.previous_mutation_id,
            object: candidate.object,
            base_object_version: candidate.base_object_version,
            payload: confirmed_candidate.payload,
            resource_refs: candidate
                .resource_refs
                .iter()
                .map(super::sync_manifests::WireResource::from)
                .collect(),
        },
        aliases: aliases
            .iter()
            .map(|alias| {
                serde_json::json!({"kind":"playlist_member","alias":{
                    "playlist_id":alias.playlist_id,
                    "alias":alias.alias_member_id,
                    "canonical":alias.canonical_member_id
                }})
            })
            .chain(song_alias.into_iter().map(|(alias, canonical)| serde_json::json!({"kind":"song","alias":{"alias":alias,"canonical":canonical}})))
            .collect(),
        resource_refs: candidate
            .resource_refs
            .iter()
            .map(super::sync_manifests::WireResource::from)
            .collect(),
    };
    let body = serde_json::to_vec(&commit_body).map_err(storage)?;
    let body_len = u32::try_from(body.len()).map_err(storage)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"echo.lan.commit.v1\0");
    hasher.update(&1u16.to_be_bytes());
    hasher.update(&sequence.as_u64().to_be_bytes());
    hasher.update(&previous_digest);
    hasher.update(&body_len.to_be_bytes());
    hasher.update(&body);
    let commit_digest = hasher.finalize().as_bytes().to_vec();
    let canonical_json = String::from_utf8(body).map_err(storage)?;
    Ok((canonical_json, commit_digest))
}

fn load_confirmed_object(
    tx: &rusqlite::Transaction<'_>,
    object: ObjectKey,
) -> Result<(bool, Option<ConfirmedObject>), Error> {
    let (kind, object_uuid) = object_identity(object);
    let current_row: Option<(i64, i64, Option<String>, String, bool)> = tx.query_row(
                "SELECT v.object_version, v.commit_seq, v.payload_json, c.origin_device_id, v.is_tombstone FROM sync_object_versions v JOIN sync_commits c ON c.commit_seq=v.commit_seq WHERE v.object_type = ?1 AND v.object_uuid = ?2",
                params![kind, object_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(storage)?;
    let current_is_tombstone = current_row.as_ref().is_some_and(|row| row.4);
    let current = current_row
        .filter(|row| !row.4)
        .map(
            |(version, _sequence, payload, author, _tombstone)| -> Result<ConfirmedObject, Error> {
                Ok(ConfirmedObject {
                    object,
                    version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(
                        u64::try_from(version).map_err(storage)?,
                    )),
                    origin_device_id: DeviceId::from_uuid(
                        uuid::Uuid::parse_str(&author).map_err(storage)?,
                    ),
                    payload: serde_json::from_str(payload.as_deref().ok_or_else(|| {
                        Error::InvariantViolation {
                            why: "live sync object has no payload".to_owned(),
                        }
                    })?)
                    .map_err(storage)?,
                })
            },
        )
        .transpose()?;
    Ok((current_is_tombstone, current))
}

fn exclude_final_playlist_members(
    tx: &rusqlite::Transaction<'_>,
    object: ObjectKey,
    current: Option<ConfirmedObject>,
) -> Result<Option<ConfirmedObject>, Error> {
    let ObjectKey::Playlist(playlist_id) = object else {
        return Ok(current);
    };
    let Some(mut current) = current else {
        return Ok(None);
    };
    let Some(members) = current
        .payload
        .get_mut("members")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(Some(current));
    };
    let mut live = Vec::with_capacity(members.len());
    for member in members.drain(..) {
        let member_id = member
            .get("member_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("confirmed playlist member is missing its identity"))?;
        let song_id = member
            .get("song_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("confirmed playlist member is missing its song"))?;
        let deleted: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type='playlist-item' AND object_uuid=?1 AND is_tombstone=1) OR EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type='song' AND object_uuid=?2 AND is_tombstone=1)",
                params![member_id, song_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let is_historical_alias: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
                params![playlist_id.to_string(), member_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !deleted && !is_historical_alias {
            live.push(member);
        }
    }
    *members = live;
    Ok(Some(current))
}

fn exclude_historical_member_aliases(
    tx: &rusqlite::Transaction<'_>,
    candidate: &MutationCandidate,
) -> Result<MutationCandidate, Error> {
    let ObjectKey::Playlist(playlist_id) = candidate.object else {
        return Ok(candidate.clone());
    };
    let mut normalized = candidate.clone();
    let Some(members) = normalized
        .payload
        .get_mut("members")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(normalized);
    };
    let mut live = Vec::with_capacity(members.len());
    for member in members.drain(..) {
        let member_id = member
            .get("member_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("playlist member is missing its identity"))?;
        let is_historical_alias: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
                params![playlist_id.to_string(), member_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !is_historical_alias {
            live.push(member);
        }
    }
    *members = live;
    Ok(normalized)
}

fn normalize_member_alias_object(
    tx: &rusqlite::Transaction<'_>,
    candidate: MutationCandidate,
) -> Result<MutationCandidate, Error> {
    let ObjectKey::PlaylistItem(member_id) = candidate.object else {
        return Ok(candidate);
    };
    let mut statement = tx
        .prepare("SELECT canonical_member_uuid FROM sync_member_aliases WHERE alias_member_uuid=?1")
        .map_err(storage)?;
    let targets = statement
        .query_map(params![member_id.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    if targets.len() > 1 {
        return Err(Error::conflict(
            "playlist member alias is ambiguous without a playlist identity",
        ));
    }
    let Some(target) = targets.first() else {
        return Ok(candidate);
    };
    let target = uuid::Uuid::parse_str(target).map_err(storage)?;
    Ok(MutationCandidate {
        object: ObjectKey::PlaylistItem(crate::domain::ids::PlaylistItemId::from_uuid(target)),
        ..candidate
    })
}

fn load_origin_cursor(
    tx: &rusqlite::Transaction<'_>,
    device: DeviceId,
) -> Result<Option<OriginCursor>, Error> {
    let cursor_row: Option<(i64, String)> = tx.query_row(
                "SELECT origin_seq, last_mutation_id FROM sync_origin_cursors WHERE origin_device_id = ?1",
                params![device.as_uuid().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(storage)?;
    let cursor = cursor_row
        .map(|(seq, id)| -> Result<OriginCursor, Error> {
            Ok(OriginCursor {
                origin_device_id: device,
                origin_seq: u64::try_from(seq).map_err(storage)?,
                last_mutation_id: MutationId::from_uuid(
                    uuid::Uuid::parse_str(&id).map_err(storage)?,
                ),
            })
        })
        .transpose()?;
    Ok(cursor)
}

#[derive(Serialize)]
struct WireMutationBody {
    protocol_version: u16,
    binding: SyncBinding,
    origin_device_id: DeviceId,
    mutation_id: MutationId,
    origin_seq: u64,
    previous_mutation_id: Option<MutationId>,
    object: ObjectKey,
    base_object_version: Option<ObjectVersion>,
    payload: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    resource_refs: Vec<super::sync_manifests::WireResource>,
}

#[derive(Serialize)]
struct WireCommitBody {
    protocol_version: u16,
    binding: SyncBinding,
    commit_seq: CommitSeq,
    previous_digest: [u8; 32],
    mutation: WireMutationBody,
    aliases: Vec<serde_json::Value>,
    resource_refs: Vec<super::sync_manifests::WireResource>,
}
