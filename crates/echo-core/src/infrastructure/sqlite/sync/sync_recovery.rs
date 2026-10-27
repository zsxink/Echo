//! Explicit host generation recovery with immutable, root-scoped old history.

use rusqlite::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use crate::application::ports::{
    HostEpochRecoveryPort, HostRecoveryArchive, HostRecoveryCandidate, RecoveredAuthorityFacts,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::sync::SyncBinding;
use crate::error::Error;

use super::super::SqliteDatabase;
use super::super::support::{now_ms, storage};

// Children precede parents. Recovery copies the exact SQL values into tables
// without foreign keys before clearing the selected authority's live lineage.
const PROTOCOL_TABLES: &[&str] = &[
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
const ROOT_TABLES: &[&str] = &[
    "sync_mutations",
    "sync_outbox",
    "sync_local_origin_cursors",
    "sync_remote_resource_locations",
    "sync_remote_publication_claims",
    "sync_remote_publications",
];

impl HostEpochRecoveryPort for SqliteDatabase {
    fn rotate_host_epoch(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        replacement: SyncBinding,
    ) -> Result<HostRecoveryArchive, Error> {
        self.recover_epoch(root, expected, replacement, None)
    }

    fn recover_unbound_host(
        &self,
        root: LibraryRootId,
        previous: SyncBinding,
        replacement: SyncBinding,
        facts: RecoveredAuthorityFacts,
    ) -> Result<HostRecoveryArchive, Error> {
        self.recover_epoch(root, previous, replacement, Some(facts))
    }

    fn host_recovery_archives(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<HostRecoveryArchive>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection.prepare("SELECT recovery_id,previous_binding_json,replacement_binding_json,pending_count,conflict_count FROM sync_recovery_archives WHERE local_root_uuid=?1 ORDER BY created_at,recovery_id").map_err(storage)?;
            let rows = statement.query_map(params![root.to_string()], |row| Ok((
                row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?, row.get::<_,i64>(3)?, row.get::<_,i64>(4)?,
            ))).map_err(storage)?;
            rows.map(|row| {
                let (id, previous, replacement, pending, conflicts) = row.map_err(storage)?;
                Ok(HostRecoveryArchive {
                    recovery_id: Uuid::parse_str(&id).map_err(storage)?, local_root: root,
                    previous_binding: serde_json::from_str(&previous).map_err(storage)?,
                    replacement_binding: serde_json::from_str(&replacement).map_err(storage)?,
                    pending_count: u64::try_from(pending).map_err(storage)?, conflict_count: u64::try_from(conflicts).map_err(storage)?,
                })
            }).collect()
        })
    }

    fn host_recovery_candidates(
        &self,
        recovery_id: Uuid,
    ) -> Result<Vec<HostRecoveryCandidate>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection.prepare("SELECT mutation_id,object_type,object_uuid,payload_json,NULL,resource_refs_json FROM recovery_sync_mutations WHERE recovery_id=?1 AND status='pending' UNION ALL SELECT conflict_id,object_type,object_uuid,replaced_payload_json,accepted_payload_json,'[]' FROM recovery_sync_conflicts WHERE recovery_id=?1 AND resolved_at IS NULL ORDER BY 1").map_err(storage)?;
            let rows = statement.query_map(params![recovery_id.to_string()], |row| Ok((
                row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,String>(5)?,
            ))).map_err(storage)?;
            rows.map(|row| {
                let (identity, object_type, object_uuid, payload_json, competing_payload_json, resource_refs_json) = row.map_err(storage)?;
                Ok(HostRecoveryCandidate { identity, object_type, object_uuid: Uuid::parse_str(&object_uuid).map_err(storage)?, payload_json, competing_payload_json, resource_refs_json })
            }).collect()
        })
    }
}

fn count(connection: &Connection, sql: &str, root: &str) -> Result<u64, Error> {
    let value: i64 = if sql.contains("?1") {
        connection
            .query_row(sql, params![root], |row| row.get(0))
            .map_err(storage)?
    } else {
        connection
            .query_row(sql, [], |row| row.get(0))
            .map_err(storage)?
    };
    u64::try_from(value).map_err(storage)
}

fn require_expected_host(
    connection: &Connection,
    root: &str,
    expected: SyncBinding,
) -> Result<(), Error> {
    let matches: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND local_root_uuid=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4)", params![
        root, expected.library_id.as_uuid().to_string(), expected.authority_id.as_uuid().to_string(), i64::try_from(expected.epoch.as_u64()).map_err(storage)?,
    ], |row| row.get(0)).map_err(storage)?;
    if matches {
        Ok(())
    } else {
        Err(Error::conflict(
            "host recovery binding changed or is not a host",
        ))
    }
}

fn archive_and_clear(connection: &Connection, receipt: &HostRecoveryArchive) -> Result<(), Error> {
    let id = receipt.recovery_id.to_string();
    let root = receipt.local_root.to_string();
    // Final deletion knowledge must enter the replacement epoch's baseline,
    // including UUIDs whose canonical rows were removed long ago.
    connection.execute("INSERT OR IGNORE INTO sync_epoch_tombstones SELECT ?1,object_type,object_uuid FROM sync_object_versions WHERE is_tombstone=1", params![root]).map_err(storage)?;
    for table in PROTOCOL_TABLES {
        connection
            .execute(
                &format!("INSERT INTO recovery_{table} SELECT ?1,live.* FROM {table} live"),
                params![id],
            )
            .map_err(storage)?;
    }
    for table in ROOT_TABLES {
        connection.execute(&format!("INSERT INTO recovery_{table} SELECT ?1,live.* FROM {table} live WHERE local_root_uuid=?2"), params![id,root]).map_err(storage)?;
    }
    for table in PROTOCOL_TABLES {
        connection
            .execute(&format!("DELETE FROM {table}"), [])
            .map_err(storage)?;
    }
    for table in ROOT_TABLES {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE local_root_uuid=?1"),
                params![root],
            )
            .map_err(storage)?;
    }
    // Typed aliases are immutable identity knowledge rather than old cursor
    // positions. Preserve them as baseline facts, with no old commit sequence.
    connection.execute("INSERT INTO sync_song_aliases SELECT alias_song_uuid,canonical_song_uuid,0 FROM recovery_sync_song_aliases WHERE recovery_id=?1", params![id]).map_err(storage)?;
    connection.execute("INSERT INTO sync_member_aliases SELECT playlist_uuid,alias_member_uuid,canonical_member_uuid,0 FROM recovery_sync_member_aliases WHERE recovery_id=?1", params![id]).map_err(storage)?;
    Ok(())
}

fn restore_portable_facts(
    connection: &Connection,
    root: &str,
    facts: &RecoveredAuthorityFacts,
) -> Result<(), Error> {
    use crate::domain::library::PortableAlias;
    use crate::domain::sync::ObjectKey;
    for key in &facts.tombstones {
        let (kind, id) = match key {
            ObjectKey::Song(id) => ("song", id.as_uuid()),
            ObjectKey::Playlist(id) => ("playlist", id.as_uuid()),
            ObjectKey::PlaylistItem(id) => ("playlist-item", id.as_uuid()),
            ObjectKey::Favorite(id) => ("favorite", id.as_uuid()),
            ObjectKey::Override(id) => ("override", id.as_uuid()),
            ObjectKey::PlayStats(id) => ("play-stats", id.as_uuid()),
            ObjectKey::LibraryRoot(id) => ("library-root", *id),
        };
        connection
            .execute(
                "INSERT OR IGNORE INTO sync_epoch_tombstones VALUES(?1,?2,?3)",
                params![root, kind, id.to_string()],
            )
            .map_err(storage)?;
    }
    for alias in &facts.aliases {
        match alias {
            PortableAlias::Song {
                alias_uuid,
                canonical_uuid,
            } => {
                connection
                    .execute(
                        "INSERT INTO sync_song_aliases VALUES(?1,?2,0)",
                        params![alias_uuid.to_string(), canonical_uuid.to_string()],
                    )
                    .map_err(storage)?;
            }
            PortableAlias::PlaylistItem {
                playlist_uuid,
                alias_uuid,
                canonical_uuid,
            } => {
                connection
                    .execute(
                        "INSERT INTO sync_member_aliases VALUES(?1,?2,?3,0)",
                        params![
                            playlist_uuid.to_string(),
                            alias_uuid.to_string(),
                            canonical_uuid.to_string()
                        ],
                    )
                    .map_err(storage)?;
            }
        }
    }
    Ok(())
}

pub(super) fn enqueue_recovered_tombstones(
    connection: &Connection,
    root: &str,
) -> Result<usize, Error> {
    let mut statement = connection.prepare("SELECT object_type,object_uuid FROM sync_epoch_tombstones WHERE local_root_uuid=?1 ORDER BY object_type,object_uuid").map_err(storage)?;
    let rows = statement
        .query_map(params![root], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for (kind, id) in &rows {
        super::enqueue::enqueue_sync_in_root(
            connection,
            root,
            kind,
            id,
            "{\"tombstone\":true}",
            "delete",
            None,
        )?;
    }
    Ok(rows.len())
}

#[cfg(test)]
#[path = "../tests/sync_recovery.rs"]
mod tests;

impl SqliteDatabase {
    fn recover_epoch(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        replacement: SyncBinding,
        unbound_facts: Option<RecoveredAuthorityFacts>,
    ) -> Result<HostRecoveryArchive, Error> {
        if replacement.library_id != expected.library_id
            || replacement.epoch <= expected.epoch
            || replacement.authority_id == expected.authority_id
        {
            return Err(Error::conflict(
                "host recovery requires a new authority and higher epoch for the same library",
            ));
        }
        let epoch = i64::try_from(replacement.epoch.as_u64()).map_err(storage)?;
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let root_string = root.to_string();
            super::context::select(&tx, &root_string)?;
            if unbound_facts.is_some() {
                let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_binding)", [], |row| row.get(0)).map_err(storage)?;
                if bound { return Err(Error::conflict("portable-only recovery cannot replace an existing sync binding")); }
            } else {
                require_expected_host(&tx, &root_string, expected)?;
            }
            let receipt = HostRecoveryArchive {
                recovery_id: Uuid::new_v4(), local_root: root,
                previous_binding: expected, replacement_binding: replacement,
                pending_count: count(&tx, "SELECT COUNT(*) FROM sync_mutations WHERE local_root_uuid=?1 AND status='pending'", &root_string)?,
                conflict_count: count(&tx, "SELECT COUNT(*) FROM sync_conflicts WHERE resolved_at IS NULL", &root_string)?,
            };
            tx.execute("INSERT INTO sync_recovery_archives VALUES(?1,?2,?3,?4,?5,?6,?7)", params![
                receipt.recovery_id.to_string(), root_string,
                serde_json::to_string(&expected).map_err(storage)?,
                serde_json::to_string(&replacement).map_err(storage)?,
                i64::try_from(receipt.pending_count).map_err(storage)?,
                i64::try_from(receipt.conflict_count).map_err(storage)?, now_ms(),
            ]).map_err(storage)?;
            archive_and_clear(&tx, &receipt)?;
            if let Some(facts) = &unbound_facts {
                restore_portable_facts(&tx, &root_string, facts)?;
            }
            tx.execute("INSERT INTO sync_binding(singleton,library_id,authority_id,epoch,role,updated_at,local_root_uuid) VALUES(1,?1,?2,?3,'host',?4,?5)", params![
                replacement.library_id.as_uuid().to_string(), replacement.authority_id.as_uuid().to_string(), epoch, now_ms(), root_string,
            ]).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(receipt)
        })
    }
}
