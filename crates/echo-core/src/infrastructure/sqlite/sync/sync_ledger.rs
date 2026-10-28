//! Durable host confirmation ledger backed by the library SQLite writer.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::application::ports::{
    HostCommitLedgerPort, HostPeerAuthorizationPort, PublishedCommit, StagedConfirmation,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::sync::{
    check_candidate_against_tombstone, confirm_candidate_at, payload_changed_at_ms,
    CommitCheckpoint, CommitSeq, Confirmation, ConfirmationError, ConfirmedObject, DeviceId,
    FinalTombstone, MutationCandidate, MutationId, ObjectKey, ObjectVersion, OriginAnchor,
    OriginCursor, SyncBinding, TombstoneDecisionError,
};
use crate::error::Error;

use super::super::support::{now_ms, storage};
use super::super::SqliteDatabase;

#[path = "sync_ledger/host_ledger.rs"]
mod host_ledger;
#[path = "sync_ledger/host_ledger_apply.rs"]
mod host_ledger_apply;

const DETACH_CONTEXT_TABLES: &[&str] = &[
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
];

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
            super::context::select(&tx, &root)?;
            for table in DETACH_CONTEXT_TABLES {
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
            tx.execute(
                "DELETE FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='state' AND (json_extract(envelope_json,'$.key.namespace.scope') IN ('bound','local_unbound','legacy_evidence') OR json_extract(envelope_json,'$.payload.schema') LIKE 'echo.sync.%')",
                params![root],
            )
            .map_err(storage)?;
            for table in [
                "recovery_sync_snapshot_pages",
                "recovery_sync_snapshot_resource_refs",
                "recovery_sync_snapshot_records",
                "recovery_sync_snapshot_sessions",
                "recovery_sync_replica_snapshot_pages",
                "recovery_sync_replica_snapshot_records",
                "recovery_sync_replica_snapshot_sessions",
                "recovery_sync_host_baselines",
                "recovery_sync_transfers",
                "recovery_sync_peer_cursors",
                "recovery_sync_conflicts",
                "recovery_sync_member_aliases",
                "recovery_sync_song_aliases",
                "recovery_sync_object_versions",
                "recovery_sync_commits",
                "recovery_sync_inbox",
                "recovery_sync_origin_cursors",
                "recovery_sync_peers",
                "recovery_sync_binding",
                "recovery_sync_mutations",
                "recovery_sync_outbox",
                "recovery_sync_local_origin_cursors",
                "recovery_sync_remote_resource_locations",
                "recovery_sync_remote_publication_claims",
                "recovery_sync_remote_publications",
            ] {
                tx.execute(
                    &format!(
                        "DELETE FROM {table} WHERE recovery_id IN (SELECT recovery_id FROM sync_recovery_archives WHERE local_root_uuid=?1)"
                    ),
                    params![root],
                )
                .map_err(storage)?;
            }
            tx.execute(
                "DELETE FROM sync_recovery_archives WHERE local_root_uuid=?1",
                params![root],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM sync_epoch_tombstones WHERE local_root_uuid=?1",
                params![root],
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
            super::context::select(&tx, &local_root_uuid)?;
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
                }
                // Selecting an already-bound archived root still changes the
                // live context and must commit even when binding is unchanged.
                tx.commit().map_err(storage)?;
                return Ok(());
            }
            tx.execute(
                "INSERT INTO sync_binding(singleton, library_id, authority_id, epoch, role, updated_at, local_root_uuid) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6)",
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
            super::context::select(&tx, &root)?;
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
                "INSERT INTO sync_binding(singleton, library_id, authority_id, epoch, role, updated_at, local_root_uuid) VALUES(1, ?1, ?2, ?3, 'replica', ?4, ?5)",
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
