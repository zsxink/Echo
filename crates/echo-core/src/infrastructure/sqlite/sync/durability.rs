//! Root-scoped recovery facts for LAN writes. No credential or endpoint tables
//! are exported. Confirmed payloads and immutable local events are kept apart.

mod commit;
mod facts;
mod publication;
#[cfg(test)]
mod tests;

use rusqlite::{params, Connection};
use serde_json::Value;
use uuid::Uuid;

use super::super::support::storage;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    DurableStateChange, LibraryBatch, StateKey, StateKind, StateNamespace, StateTarget,
};
use crate::error::Error;

/// Capture only immutable allocations created after the verified planning baseline.
pub(crate) fn capture_local_object_mutation(
    connection: &Connection,
    root: LibraryRootId,
    object: crate::domain::sync::ObjectKey,
    library_id: crate::domain::library::LibraryId,
    author: crate::domain::sync::DeviceId,
    after_sequence: u64,
) -> Result<Vec<DurableStateChange>, Error> {
    let namespace = facts::context_namespace(connection, &root.to_string(), true)?
        .unwrap_or(StateNamespace::LocalUnbound { library_id, author });
    let (kind, object_uuid) = super::apply::object_identity(object);
    let mut statement = connection.prepare(
        "SELECT json_object('schema','echo.sync.mutation.v1','mutation_id',mutation_id,'author',origin_device_id,'sequence',origin_seq,'predecessor',previous_mutation_id,'object_type',object_type,'object_uuid',object_uuid,'operation',operation,'revision',local_revision,'base_version',base_object_version,'payload',json(payload_json),'payload_digest',payload_digest,'original_body',payload_json,'original_resources_body',resource_refs_json,'status',status,'outcome',CASE WHEN outcome_json IS NULL THEN NULL ELSE json(outcome_json) END,'changed_at_ms',created_at,'resources',json(resource_refs_json)) FROM sync_mutations WHERE local_root_uuid=?1 AND object_type=?5 AND object_uuid=?2 AND origin_device_id=?3 AND origin_seq>?4 ORDER BY origin_seq",
    ).map_err(storage)?;
    let rows = statement
        .query_map(
            params![
                root.to_string(),
                object_uuid,
                author.as_uuid().to_string(),
                after_sequence,
                kind,
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    if rows.len() != 1 {
        return Err(Error::conflict(
            "playlist edit must freeze exactly one new mutation",
        ));
    }
    let payload: Value = serde_json::from_str(&rows[0]).map_err(storage)?;
    if payload["status"].as_str() != Some("pending")
        || !matches!(payload["operation"].as_str(), Some("upsert" | "delete"))
    {
        return Err(Error::conflict(
            "playlist edit allocated unexpected mutation",
        ));
    }
    Ok(facts::mutation(&payload, namespace)?.into_iter().collect())
}

/// Freeze a complete planned playlist body, including its original-asset reference.
pub(crate) fn allocate_local_playlist_snapshot(
    connection: &Connection,
    root: LibraryRootId,
    playlist: crate::domain::ids::PlaylistId,
    payload: &Value,
) -> Result<(), Error> {
    let revision = super::enqueue::enqueue_sync_in_root(
        connection,
        &root.to_string(),
        "playlist",
        &playlist.to_string(),
        &serde_json::to_string(payload).map_err(storage)?,
        super::OP_UPSERT,
        None,
    )?;
    super::enqueue::mirror_playlist_revision(connection, playlist, revision)
}

/// Allocate a deletion from a root already verified by the local edit planner.
/// Retained orphan members need not have a canonical SQL row to own this fact.
pub(crate) fn allocate_local_delete(
    connection: &Connection,
    root: LibraryRootId,
    object: crate::domain::sync::ObjectKey,
) -> Result<u64, Error> {
    let (kind, id) = super::apply::object_identity(object);
    let revision = super::enqueue::enqueue_sync_in_root(
        connection,
        &root.to_string(),
        kind,
        &id,
        "{\"tombstone\":true}",
        super::OP_DELETE,
        None,
    )?;
    u64::try_from(revision).map_err(storage)
}

/// Stable, namespace-separated local fact identity; it is not a LAN mutation.
pub(crate) fn identity(namespace: &str, identity: &str) -> Uuid {
    let mut hash = blake3::Hasher::new();
    hash.update(namespace.as_bytes());
    hash.update(&[0]);
    hash.update(identity.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}

fn values(connection: &Connection, sql: &str, root: &str) -> Result<Vec<Value>, Error> {
    let mut statement = connection.prepare(sql).map_err(storage)?;
    let rows = statement
        .query_map(params![root], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage))
        .collect()
}

/// Portable sync facts, scoped to one local library. The active protocol context
/// and archived contexts share a layout, but trust is intentionally not restored.
pub(crate) fn export_sync_state(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<DurableStateChange>, Error> {
    let root_string = root.to_string();
    let mut state = super::super::durability_backfill::export_playback_events(connection, root)?;
    // Every mutation keeps its exact original body, author, predecessor, frozen
    // time and revision, including terminal outcomes. A new author cannot send it.
    for payload in values(connection, "SELECT json_object('schema','echo.sync.mutation.v1','mutation_id',mutation_id,'author',origin_device_id,'sequence',origin_seq,'predecessor',previous_mutation_id,'object_type',object_type,'object_uuid',object_uuid,'operation',operation,'revision',local_revision,'base_version',base_object_version,'payload',json(payload_json),'payload_digest',payload_digest,'original_body',payload_json,'original_resources_body',resource_refs_json,'status',status,'outcome',CASE WHEN outcome_json IS NULL THEN NULL ELSE json(outcome_json) END,'changed_at_ms',created_at,'resources',json(resource_refs_json)) FROM sync_mutations WHERE local_root_uuid=?1 ORDER BY origin_device_id,origin_seq,mutation_id", &root_string)? {
        // These historical rows have no freeze-time binding column. Never
        // infer one from today's selected context or credentials.
        state.extend(facts::mutation(&payload, StateNamespace::LegacyEvidence { source_root: root })?);
    }
    let active: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_active_context WHERE singleton=1 AND local_root_uuid=?1)", params![root_string], |row| row.get(0)).map_err(storage)?;
    // These tables are already scoped by the selected binding. Never export
    // another root's active ledger while backing up an inactive local library.
    let prefix = if active { "" } else { "archived_" };
    let scope = if active {
        "?1 IS NOT NULL"
    } else {
        "context_root=?1"
    };
    let context_namespace = facts::context_namespace(connection, &root_string, active)?;
    let queries = [
        (StateKind::Confirmed, "object", format!("SELECT json_object('schema','echo.sync.confirmed.v1','object_type',object_type,'object_uuid',object_uuid,'version',object_version,'sequence',commit_seq,'payload',CASE WHEN payload_json IS NULL THEN NULL ELSE json(payload_json) END,'tombstone',json(CASE WHEN is_tombstone=1 THEN 'true' ELSE 'false' END),'changed_at_ms',changed_at_ms) FROM {prefix}sync_object_versions WHERE {scope} ORDER BY object_type,object_uuid")),
        (StateKind::Alias, "song-alias", format!("SELECT json_object('schema','echo.sync.song-alias.v1','alias',alias_song_uuid,'canonical',canonical_song_uuid,'sequence',commit_seq) FROM {prefix}sync_song_aliases WHERE {scope} ORDER BY alias_song_uuid")),
        (StateKind::Alias, "member-alias", format!("SELECT json_object('schema','echo.sync.member-alias.v1','playlist',playlist_uuid,'alias',alias_member_uuid,'canonical',canonical_member_uuid,'sequence',commit_seq) FROM {prefix}sync_member_aliases WHERE {scope} ORDER BY playlist_uuid,alias_member_uuid")),
        (StateKind::SyncLedger, "commit", format!("SELECT {} FROM {prefix}sync_commits WHERE {scope} ORDER BY commit_seq", publication::LEDGER_PAYLOAD)),
        (StateKind::Receipt, "receipt", format!("SELECT json_object('schema','echo.sync.receipt.v1','author',origin_device_id,'mutation_id',mutation_id,'payload_digest',payload_digest,'outcome',json(outcome_json),'original_outcome',outcome_json,'received_at',received_at,'sequence',commit_seq) FROM {prefix}sync_inbox WHERE {scope} ORDER BY origin_device_id,mutation_id")),
        (StateKind::Cursor, "origin", format!("SELECT json_object('schema','echo.sync.origin-cursor.v1','author',origin_device_id,'sequence',origin_seq,'mutation_id',last_mutation_id) FROM {prefix}sync_origin_cursors WHERE {scope} ORDER BY origin_device_id")),
        (StateKind::Cursor, "peer", format!("SELECT json_object('schema','echo.sync.peer-cursor.v1','peer',peer_device_id,'sequence',applied_commit_seq,'digest',lower(hex(applied_digest))) FROM {prefix}sync_peer_cursors WHERE {scope} ORDER BY peer_device_id")),
        (StateKind::Conflict, "conflict", format!("SELECT json_object('schema','echo.sync.conflict.v1','id',conflict_id,'object_type',object_type,'object_uuid',object_uuid,'replaced_version',replaced_version,'accepted_version',accepted_version,'replaced',json(replaced_payload_json),'accepted',json(accepted_payload_json),'resolved_at',resolved_at) FROM {prefix}sync_conflicts WHERE {scope} ORDER BY conflict_id")),
    ];
    for (kind, namespace, query) in queries {
        for payload in values(connection, &query, &root_string)? {
            let namespace_scope = if namespace == "commit" {
                StateNamespace::Bound {
                    binding: facts::binding(&payload)?,
                }
            } else {
                context_namespace
                    .clone()
                    .unwrap_or(StateNamespace::LegacyEvidence { source_root: root })
            };
            let target = facts::target(namespace, &payload)?;
            debug_assert_eq!(kind, target.kind());
            let key = StateKey {
                namespace: namespace_scope,
                target,
            };
            if namespace == "receipt" {
                // SQL inbox rows have no schema marker: host confirmations may
                // be complete, while replica/legacy outcomes are evidence only.
                let mut complete = payload.clone();
                complete["schema"] = Value::from("echo.sync.receipt.v2");
                if let Ok(receipt) = DurableStateChange::scoped(key.clone(), complete) {
                    state.push(receipt);
                    continue;
                }
            }
            state.push(DurableStateChange::scoped(key, payload)?);
        }
    }
    complete_confirmations(&mut state)?;
    // Physical associations only become ready after verified publication. The
    // full publication also preserves pending claims and their retained bytes.
    for mut payload in values(connection, "SELECT json_object('schema','echo.sync.resource-publication.v1','operation',operation_uuid,'publication',json(publication_json),'original_body',publication_json,'completed',json(CASE WHEN completed=1 THEN 'true' ELSE 'false' END)) FROM sync_remote_publications WHERE local_root_uuid=?1 ORDER BY operation_uuid", &root_string)? {
        // Publication rows predate binding capture as well; root provenance
        // retains their exact operation without asserting a historical binding.
        let target = StateTarget::Publication { operation_id: parse_uuid(&payload,"operation")? };
        let key = StateKey { namespace: StateNamespace::LegacyEvidence { source_root: root }, target };
        payload["schema"] = Value::from("echo.sync.resource-publication.v2");
        if let Ok(complete) = DurableStateChange::scoped(key.clone(), payload.clone()) {
            state.push(complete);
        } else {
            // Old layouts or incomplete journals remain unproven evidence. Do
            // not retrofit today's owner/version or alter committed v1 facts.
            payload["schema"] = Value::from("echo.sync.resource-publication.v1");
            payload.as_object_mut().ok_or_else(|| Error::conflict("invalid publication evidence"))?.remove("original_body");
            state.push(DurableStateChange::scoped(key, payload)?);
        }
    }
    for payload in values(connection, "SELECT json_object('schema','echo.sync.local-origin.v1','author',origin_device_id,'sequence',origin_seq,'mutation_id',last_mutation_id) FROM sync_local_origin_cursors WHERE local_root_uuid=?1 ORDER BY origin_device_id", &root_string)? {
        let target = StateTarget::LocalOriginCursor { author: crate::domain::sync::DeviceId::from_uuid(parse_uuid(&payload,"author")?) };
        state.push(DurableStateChange::scoped(StateKey { namespace: StateNamespace::LegacyEvidence { source_root: root }, target }, payload)?);
    }
    Ok(state)
}

fn complete_confirmations(states: &mut [DurableStateChange]) -> Result<(), Error> {
    let ledgers: std::collections::BTreeMap<_, _> = states
        .iter()
        .filter(|state| state.kind == StateKind::SyncLedger)
        .map(|state| {
            Ok((
                (
                    serde_json::to_string(&state.key.as_ref().map(|key| &key.namespace))
                        .map_err(storage)?,
                    state.payload["sequence"].as_u64(),
                ),
                state.payload.clone(),
            ))
        })
        .collect::<Result<_, Error>>()?;
    for state in states
        .iter_mut()
        .filter(|state| state.kind == StateKind::Confirmed)
    {
        let Some(key) = state.key.clone() else {
            continue;
        };
        let namespace = serde_json::to_string(&Some(&key.namespace)).map_err(storage)?;
        let Some(ledger) = ledgers.get(&(namespace, state.payload["sequence"].as_u64())) else {
            continue;
        };
        let mut payload = state.payload.clone();
        payload["schema"] = Value::from("echo.sync.confirmed.v2");
        if payload["tombstone"] == Value::Bool(true) && payload["payload"].is_null() {
            payload["payload"] = serde_json::json!({"tombstone":true});
        }
        for field in [
            "author",
            "mutation_id",
            "original_body",
            "previous_digest",
            "digest",
        ] {
            payload[field] = ledger[field].clone();
        }
        // Incomplete or conflicting historical evidence stays v1, never gains
        // a guessed event body or today's identity. Already committed v1 stays intact.
        if let Ok(complete) = DurableStateChange::scoped(key, payload) {
            *state = complete;
        }
    }
    Ok(())
}

pub(crate) use commit::{
    commit_projection, commit_publication_projection, commit_resource_projection,
};

pub(crate) fn active_root(connection: &Connection) -> Result<LibraryRootId, Error> {
    let root: String = connection
        .query_row(
            "SELECT local_root_uuid FROM sync_active_context WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    Uuid::parse_str(&root)
        .map(LibraryRootId::from_uuid)
        .map_err(storage)
}
fn text<'a>(payload: &'a Value, name: &str) -> Result<&'a str, Error> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict(format!("invalid sync recovery {name}")))
}
fn parse_uuid(payload: &Value, name: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(text(payload, name)?).map_err(storage)
}

/// Capture the actual events allocated by a planned SQL projection. This
/// runs before the directory commit; reconstructing candidates later would lose
/// original IDs, predecessors, base versions, edit times and immutable bodies.
pub(crate) fn enrich_recovery_batch(
    connection: &Connection,
    root: LibraryRootId,
    batch: &mut LibraryBatch,
) -> Result<(), Error> {
    let exact = export_sync_state(connection, root)?;
    let mut positions = std::collections::BTreeMap::new();
    for (index, state) in batch.state.iter().enumerate() {
        // Match the original linear find when input already contains duplicates.
        positions.entry((state.kind, state.id)).or_insert(index);
    }
    for fact in exact {
        if let Some(&index) = positions.get(&(fact.kind, fact.id)) {
            batch.state[index] = fact;
        } else {
            positions.insert((fact.kind, fact.id), batch.state.len());
            batch.state.push(fact);
        }
    }
    Ok(())
}
