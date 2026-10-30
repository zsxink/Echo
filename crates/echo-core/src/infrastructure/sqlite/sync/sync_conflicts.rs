//! Host-local conflict recovery; no historical structural payload is replayed.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::super::support::{now_ms, storage};
use super::super::SqliteDatabase;
use crate::application::ports::{
    HostSyncConflictPort, SyncConflictRecovery, SyncConflictResolution, SyncConflictView,
};
use crate::domain::ids::{LibraryRootId, PlaylistId};
use crate::domain::sync::SyncBinding;
use crate::error::Error;

fn require_host(
    connection: &Connection,
    root: LibraryRootId,
    expected: SyncBinding,
) -> Result<(), Error> {
    let valid: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_binding b JOIN sync_active_context c ON c.local_root_uuid=b.local_root_uuid JOIN library_roots r ON r.uuid=b.local_root_uuid WHERE b.singleton=1 AND b.role='host' AND b.local_root_uuid=?1 AND b.library_id=?2 AND b.authority_id=?3 AND b.epoch=?4 AND r.is_active=1)",
        params![root.to_string(), expected.library_id.as_uuid().to_string(), expected.authority_id.as_uuid().to_string(), i64::try_from(expected.epoch.as_u64()).map_err(storage)?], |row| row.get(0),
    ).map_err(storage)?;
    if valid {
        Ok(())
    } else {
        Err(Error::conflict(
            "conflict recovery requires the current active host authority",
        ))
    }
}

fn fields(kind: &str, payload: &Value) -> Value {
    let names: &[&str] = match kind {
        "playlist" => &["display_name"],
        "override" => &["title", "artist", "album", "lyrics_text"],
        "favorite" => &["is_favorite"],
        "play-stats" => &["by_device"],
        _ => &[],
    };
    Value::Object(
        names
            .iter()
            .filter_map(|name| {
                payload
                    .get(name)
                    .map(|value| ((*name).to_owned(), value.clone()))
            })
            .collect::<Map<_, _>>(),
    )
}

fn has_user_value_change(kind: &str, replaced: &Value, accepted: &Value) -> bool {
    match kind {
        "favorite" => replaced.get("is_favorite") != accepted.get("is_favorite"),
        "play-stats" => replaced.get("by_device") != accepted.get("by_device"),
        _ => true,
    }
}

fn recovery(
    connection: &Connection,
    root: LibraryRootId,
    kind: &str,
    object: &str,
) -> Result<SyncConflictRecovery, Error> {
    let deleted: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE object_uuid=?1 AND is_tombstone=1 AND (object_type=?2 OR (object_type='song' AND ?2='override'))) OR EXISTS(SELECT 1 FROM tombstones WHERE object_uuid=?1 AND (object_type=?2 OR (object_type='song' AND ?2='override'))) OR EXISTS(SELECT 1 FROM sync_epoch_tombstones WHERE local_root_uuid=?3 AND object_uuid=?1 AND (object_type=?2 OR (object_type='song' AND ?2='override')))",
        params![object, kind, root.to_string()], |row| row.get(0),
    ).map_err(storage)?;
    if deleted {
        return Ok(SyncConflictRecovery::FinalDeleted);
    }
    if !matches!(kind, "playlist" | "override") {
        return Ok(SyncConflictRecovery::Unsupported);
    }
    let table = if kind == "playlist" {
        "playlists"
    } else {
        "songs"
    };
    let exists: bool = connection
        .query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE uuid=?1 AND library_root_uuid=?2)"),
            params![object, root.to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !exists {
        return Ok(SyncConflictRecovery::Unavailable);
    }
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND object_uuid=?2 AND status='pending' AND (object_type=?3 OR (object_type='song' AND ?3='override')))", params![root.to_string(), object, kind], |row| row.get(0)).map_err(storage)?;
    if pending {
        Ok(SyncConflictRecovery::PendingEdit)
    } else {
        Ok(SyncConflictRecovery::Available)
    }
}

impl HostSyncConflictPort for SqliteDatabase {
    fn host_sync_conflicts(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        limit: usize,
    ) -> Result<Vec<SyncConflictView>, Error> {
        self.with_reader(move |connection| {
            // An explicit read transaction keeps the authority check and list
            // on one snapshot while another writer activates a different root.
            let tx = connection.unchecked_transaction().map_err(storage)?;
            require_host(&tx, root, expected)?;
            let mut statement = tx.prepare("SELECT conflict_id,object_type,object_uuid,replaced_version,accepted_version,replaced_payload_json,accepted_payload_json FROM sync_conflicts WHERE resolved_at IS NULL ORDER BY accepted_version DESC,conflict_id").map_err(storage)?;
            let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, String>(5)?, row.get::<_, String>(6)?))).map_err(storage)?;
            let mut conflicts = Vec::new();
            for row in rows {
                if conflicts.len() == limit.clamp(1, 200) {
                    break;
                }
                let (id, kind, object, replaced_version, accepted_version, replaced, accepted) = row.map_err(storage)?;
                let replaced: Value = serde_json::from_str(&replaced).map_err(storage)?;
                let accepted: Value = serde_json::from_str(&accepted).map_err(storage)?;
                // Filter no-op conflicts already persisted by older versions;
                // metadata-only revision changes are not user-facing conflicts.
                if !has_user_value_change(&kind, &replaced, &accepted) {
                    continue;
                }
                conflicts.push(SyncConflictView {
                    conflict_id: Uuid::parse_str(&id).map_err(storage)?, object_uuid: Uuid::parse_str(&object).map_err(storage)?,
                    replaced_version: u64::try_from(replaced_version).map_err(storage)?, accepted_version: u64::try_from(accepted_version).map_err(storage)?,
                    replaced_fields: fields(&kind, &replaced), accepted_fields: fields(&kind, &accepted),
                    recovery: recovery(&tx, root, &kind, &object)?, object_type: kind,
                });
            }
            Ok(conflicts)
        })
    }

    fn resolve_host_sync_conflict(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        conflict: Uuid,
        recover_replaced: bool,
    ) -> Result<SyncConflictResolution, Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            require_host(&tx, root, expected)?;
            let row: Option<(String, String, String, Option<i64>)> = tx.query_row("SELECT object_type,object_uuid,replaced_payload_json,resolved_at FROM sync_conflicts WHERE conflict_id=?1", [conflict.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(storage)?;
            let Some((kind, object, replaced, resolved)) = row else { return Ok(SyncConflictResolution::Unavailable); };
            if resolved.is_some() { return Ok(SyncConflictResolution::AlreadyResolved); }
            if recover_replaced {
                match recovery(&tx, root, &kind, &object)? {
                    SyncConflictRecovery::Available => {},
                    SyncConflictRecovery::PendingEdit => return Ok(SyncConflictResolution::PendingEdit),
                    SyncConflictRecovery::FinalDeleted => return Ok(SyncConflictResolution::FinalDeleted),
                    SyncConflictRecovery::Unsupported => return Ok(SyncConflictResolution::Unsupported),
                    SyncConflictRecovery::Unavailable => return Ok(SyncConflictResolution::Unavailable),
                }
                let replaced: Value = serde_json::from_str(&replaced).map_err(storage)?;
                let mut payload = if kind == "playlist" {
                    super::super::statements::playlist_full_snapshot_payload(&tx, PlaylistId::from_uuid(Uuid::parse_str(&object).map_err(storage)?))?
                } else {
                    let json: Option<String> = tx.query_row("SELECT payload_json FROM sync_object_versions WHERE object_type='override' AND object_uuid=?1 AND is_tombstone=0", [&object], |row| row.get(0)).optional().map_err(storage)?.flatten();
                    let Some(json) = json else { return Ok(SyncConflictResolution::Unavailable); };
                    serde_json::from_str(&json).map_err(storage)?
                };
                let values = fields(&kind, &replaced);
                if values.as_object().is_none_or(Map::is_empty) { return Ok(SyncConflictResolution::Unsupported); }
                for (name, value) in values.as_object().expect("fields is an object") {
                    if !(value.is_null() || value.is_string()) { return Err(Error::conflict("historical ordinary value has an invalid shape")); }
                    payload[name] = value.clone();
                }
                if kind == "override" {
                    let revision = super::enqueue::current_outbox_revision(&tx, &kind, &object)?.checked_add(1).ok_or_else(|| Error::conflict("local revision exhausted"))?;
                    let device: String = tx.query_row("SELECT value FROM device_state WHERE key='device_id'", [], |row| row.get(0)).map_err(storage)?;
                    let clock = super::super::statements::advance_object_hlc(&tx, &kind, &object, u64::try_from(now_ms().max(0)).map_err(storage)? / 1000)?;
                    payload["revision"] = serde_json::json!(revision);
                    payload["updated_by_device_id"] = Value::String(device);
                    payload["hlc"] = serde_json::to_value(clock).map_err(storage)?;
                }
                super::projection::project_confirmed_song_state(&tx, &kind, &object, &payload, Some(&root.to_string()))?;
                super::super::statements::enqueue_object_snapshot(&tx, &kind, &object, &payload, super::enqueue::OP_UPSERT)?;
            }
            tx.execute("UPDATE sync_conflicts SET resolved_at=?2 WHERE conflict_id=?1 AND resolved_at IS NULL", params![conflict.to_string(), now_ms()]).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(if recover_replaced { SyncConflictResolution::Recovered } else { SyncConflictResolution::Dismissed })
        })
    }
}

#[cfg(test)]
#[path = "../tests/sync_conflicts.rs"]
mod tests;
