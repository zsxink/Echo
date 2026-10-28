//! Stable typed storage, dependency resolution and path-free recovery reporting.
mod artist_cover;
mod dependencies;
mod local_allocation;
mod resource_publication;
use super::super::support::storage;
use crate::domain::{
    ids::LibraryRootId,
    library::{
        PortableRecord, PortableRecordEnvelope,
        durability::{
            DurableResource, DurableStateChange, LibraryProjectionStatus, LibraryRecoveryRecords,
            PlaybackEvent, ProjectionTarget, ProjectionUnresolvedReason as Reason, StateKind,
            UnresolvedProjectionRecord,
        },
    },
};
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn save(
    c: &Connection,
    root: LibraryRootId,
    records: &[PortableRecordEnvelope],
    states: &[DurableStateChange],
    resources: &[DurableResource],
) -> Result<(), Error> {
    for envelope in records {
        let key = serde_json::to_string(&envelope.record().object_key()?).map_err(storage)?;
        let old: Option<String> = c.query_row("SELECT envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND object_key=?2", params![root.to_string(),key], |r| r.get(0)).optional().map_err(storage)?;
        let mut effective = envelope.clone();
        if let Some(old) = old {
            let old: PortableRecordEnvelope = serde_json::from_str(&old).map_err(storage)?;
            if old.record().is_tombstone() && !envelope.record().is_tombstone() {
                continue;
            }
            if let (PortableRecord::PlayStats(before), PortableRecord::PlayStats(after)) =
                (old.record(), envelope.record())
            {
                let mut merged = after.clone();
                for (device, count) in &before.by_device {
                    let bucket = merged.by_device.entry(*device).or_default();
                    *bucket = (*bucket).max(*count);
                }
                effective = PortableRecordEnvelope::new(
                    PortableRecord::PlayStats(merged),
                    envelope.sync_metadata().cloned(),
                )?;
            }
        }
        put(
            c,
            root,
            &key,
            "record",
            &serde_json::to_string(&effective).map_err(storage)?,
        )?;
    }
    for state in states {
        put(
            c,
            root,
            &state_key(state)?,
            "state",
            &serde_json::to_string(state).map_err(storage)?,
        )?;
    }
    for resource in resources {
        put(
            c,
            root,
            &format!("resource:{}", resource.relative_path),
            "resource",
            &serde_json::to_string(resource).map_err(storage)?,
        )?;
    }
    Ok(())
}
fn put(
    c: &Connection,
    root: LibraryRootId,
    key: &str,
    kind: &str,
    json: &str,
) -> Result<(), Error> {
    if kind == "state" && is_sync_protocol_state(json)? && !has_sync_binding(c, root)? {
        // Detached/standalone roots do not retain LAN protocol facts, even when
        // an older immutable journal still contains them.
        return Ok(());
    }
    let pending = match kind {
        "record" => Some(Reason::MissingSyncDependency),
        "state" => Some(Reason::RequiresStateRecovery),
        _ => None,
    }
    .map(|reason| serde_json::to_string(&reason))
    .transpose()
    .map_err(storage)?;
    c.execute("INSERT INTO library_recovery_records(library_root_uuid,object_key,envelope_json,row_kind,unresolved_reason) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(library_root_uuid,object_key) DO UPDATE SET envelope_json=excluded.envelope_json,row_kind=excluded.row_kind,unresolved_reason=CASE WHEN library_recovery_records.envelope_json=excluded.envelope_json AND library_recovery_records.row_kind=excluded.row_kind THEN library_recovery_records.unresolved_reason ELSE excluded.unresolved_reason END",params![root.to_string(),key,json,kind,pending]).map_err(storage)?;
    Ok(())
}
pub(super) fn state_key(state: &DurableStateChange) -> Result<String, Error> {
    let target = state.key.as_ref().map_or_else(
        || ProjectionTarget::State {
            state_kind: state.kind,
            id: state.id,
        },
        |key| ProjectionTarget::ScopedState { key: key.clone() },
    );
    serde_json::to_string(&target).map_err(storage)
}
pub(super) fn reason(
    c: &Connection,
    root: LibraryRootId,
    key: &str,
    value: Option<Reason>,
) -> Result<(), Error> {
    let value = value
        .map(|v| serde_json::to_string(&v))
        .transpose()
        .map_err(storage)?;
    c.execute("UPDATE library_recovery_records SET unresolved_reason=?3 WHERE library_root_uuid=?1 AND object_key=?2 AND unresolved_reason IS NOT ?3",params![root.to_string(),key,value]).map_err(storage)?;
    Ok(())
}
pub(super) fn read(c: &Connection, root: LibraryRootId) -> Result<LibraryRecoveryRecords, Error> {
    let mut statement = c.prepare("SELECT row_kind,envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 ORDER BY object_key").map_err(storage)?;
    let rows = statement
        .query_map([root.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    let mut result = LibraryRecoveryRecords::default();
    for row in rows {
        let (kind, json) = row.map_err(storage)?;
        match kind.as_str() {
            "record" => result
                .records
                .push(serde_json::from_str(&json).map_err(storage)?),
            "state" => result
                .state
                .push(serde_json::from_str(&json).map_err(storage)?),
            "resource" => result
                .resources
                .push(serde_json::from_str(&json).map_err(storage)?),
            _ => return Err(Error::conflict("unknown retained recovery row kind")),
        }
    }
    Ok(result)
}
pub(super) fn artist_cover_original(
    c: &Connection,
    root: LibraryRootId,
    artist_key: &str,
) -> Result<Option<crate::domain::library::PortableAssetReference>, Error> {
    artist_cover::read(c, root, artist_key)
}
pub(super) fn retry(
    c: &Connection,
    root: LibraryRootId,
    library: Option<crate::domain::library::LibraryId>,
) -> Result<(), Error> {
    let sync_bound = has_sync_binding(c, root)?;
    if !sync_bound {
        // Purge protocol facts written by earlier versions on the next ordinary
        // projection. The directory journal remains the authority for local
        // catalog records, but detached LAN state is not part of a local view.
        purge_standalone_sync_data(c, root)?;
        c.execute(
            "DELETE FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='state' AND (json_extract(envelope_json,'$.key.namespace.scope') IN ('bound','local_unbound','legacy_evidence') OR json_extract(envelope_json,'$.payload.schema') LIKE 'echo.sync.%')",
            [root.to_string()],
        )
        .map_err(storage)?;
    }
    let retained = read(c, root)?;
    let mut statement = c.prepare(
        "SELECT object_key FROM library_recovery_records WHERE library_root_uuid=?1 AND unresolved_reason IS NOT NULL",
    ).map_err(storage)?;
    let pending = statement
        .query_map([root.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .map_err(storage)?;
    let pending = pending;
    let mut retry_records = Vec::new();
    for envelope in &retained.records {
        let key = serde_json::to_string(&envelope.record().object_key()?).map_err(storage)?;
        if pending.contains(&key) {
            retry_records.push(envelope.clone());
        }
    }
    retry_records.sort_by_key(|e| match e.record() {
        PortableRecord::Tombstone(_) => 0,
        PortableRecord::Song(_) => 1,
        PortableRecord::Playlist(_) => 2,
        _ => 3,
    });
    // An envelope whose `sync` block claims a host version the verified ledger
    // cannot prove stays a retained candidate. The local value is still
    // projected (the directory is the source of truth), but the row keeps
    // reporting the unproven claim so nothing treats it as a confirmed
    // baseline. The bytes themselves are never rewritten here.
    let mut unproven = std::collections::BTreeSet::new();
    for envelope in &retry_records {
        if super::confirmation::unproven(c, envelope)? {
            unproven
                .insert(serde_json::to_string(&envelope.record().object_key()?).map_err(storage)?);
        }
    }
    // Release positions for the retained membership identities as a set, so
    // valid reorderings (including swaps) do not collide with their old slots.
    // The row itself survives the release, parked at the end of its playlist:
    // its saved added_at and identity stay replaceable by the re-projection
    // instead of being discarded. A member whose dependencies are still
    // unresolved keeps its projected row: retrying must never discard a saved
    // fact it cannot replace.
    for envelope in &retry_records {
        if let PortableRecord::PlaylistItem(item) = envelope.record() {
            if dependencies::unresolved(c, root, envelope, &retained.records)?.is_none() {
                c.execute("UPDATE playlist_songs SET position=(SELECT COALESCE(MAX(parked.position),-1)+1 FROM playlist_songs parked WHERE parked.playlist_uuid=playlist_songs.playlist_uuid) WHERE member_uuid=?1 AND playlist_uuid IN (SELECT uuid FROM playlists WHERE library_root_uuid=?2)", params![item.item_uuid.to_string(),root.to_string()]).map_err(storage)?;
            }
        }
    }
    for envelope in &retry_records {
        let key = serde_json::to_string(&envelope.record().object_key()?).map_err(storage)?;
        let unresolved = dependencies::unresolved(c, root, envelope, &retained.records)?;
        if unresolved.is_none() {
            super::records::project_record(c, root, envelope)?;
        }
        let reported = unresolved.or_else(|| {
            unproven
                .contains(&key)
                .then_some(Reason::UnverifiedConfirmationMetadata)
        });
        reason(c, root, &key, reported)?;
    }
    let mut allocations = local_allocation::AllocationIndex::new(&retained.state)?;
    for state in &retained.state {
        let key = state_key(state)?;
        // Sync protocol facts are not library-projection work in single-device
        // mode. Keep their retained evidence for compatibility, but clear any
        // old catalog-recovery marker and leave them to sync-specific flows.
        if state.key.as_ref().is_some_and(|key| {
            matches!(
                &key.namespace,
                crate::domain::library::durability::StateNamespace::Bound { .. }
                    | crate::domain::library::durability::StateNamespace::LegacyEvidence { .. }
                    | crate::domain::library::durability::StateNamespace::LocalUnbound { .. }
            )
        }) {
            if sync_bound {
                // A bound protocol fact is consumed only by a validated sync
                // recovery, which owns its unresolved marker.
                continue;
            }
            // Detached protocol facts were removed before reading `retained`;
            // this guard only protects legacy rows from unexpected JSON shape.
            continue;
        }
        // Only newly written or still-pending local state rows are retried.
        if !pending.contains(&key) {
            continue;
        }
        let unresolved = if state.kind == StateKind::Claim
            && state.payload["version"] == 1
            && state.payload["kind"] == "ingestion"
            && state.payload["active"] == false
        {
            // Completed import/scan claims are retained as immutable recovery
            // evidence, but they no longer represent work to recover. Keeping
            // them out of the unresolved projection prevents one false issue
            // per ingested song from accumulating forever.
            None
        } else if state.kind == StateKind::ResourceLocation {
            resource_publication::project(c, root, library, state, &retained)?
        } else if state.kind == StateKind::ArtistCover {
            artist_cover::project(c, root, library, state)?
        } else if state.kind == StateKind::PlaybackEvent {
            let event: PlaybackEvent =
                serde_json::from_value(state.payload.clone()).map_err(storage)?;
            let dependency = dependencies::song(c, root, event.song_uuid, &retained.records)?;
            if dependency.is_none() {
                let existing: Option<String> = c.query_row("SELECT song_uuid FROM recorded_play_sessions WHERE playback_session_uuid=?1",[state.id.to_string()], |r| r.get(0)).optional().map_err(storage)?;
                if existing
                    .as_ref()
                    .is_some_and(|id| id != &event.song_uuid.to_string())
                {
                    Some(Reason::IdentityConflict)
                } else {
                    c.execute("INSERT OR IGNORE INTO recorded_play_sessions(playback_session_uuid,song_uuid,recorded_at) VALUES(?1,?2,?3)",params![state.id.to_string(),event.song_uuid.to_string(),event.recorded_at]).map_err(storage)?;
                    None
                }
            } else {
                dependency
            }
        } else {
            local_allocation::restore_indexed(c, root, library, state, &mut allocations)?
        };
        reason(c, root, &key, unresolved)?;
    }
    Ok(())
}

fn has_sync_binding(c: &Connection, root: LibraryRootId) -> Result<bool, Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND local_root_uuid=?1) OR EXISTS(SELECT 1 FROM archived_sync_binding WHERE context_root=?1 AND local_root_uuid=?1)",
        [root.to_string()],
        |row| row.get(0),
    )
    .map_err(storage)
}

fn purge_standalone_sync_data(c: &Connection, root: LibraryRootId) -> Result<(), Error> {
    let root = root.to_string();
    for table in [
        "sync_outbox",
        "sync_mutations",
        "sync_local_origin_cursors",
        "sync_remote_resource_locations",
        "sync_remote_publication_claims",
        "sync_remote_publications",
    ] {
        c.execute(
            &format!("DELETE FROM {table} WHERE local_root_uuid=?1"),
            [root.as_str()],
        )
        .map_err(storage)?;
    }
    c.execute(
        "DELETE FROM sync_epoch_tombstones WHERE local_root_uuid=?1",
        [root.as_str()],
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
        c.execute(
            &format!(
                "DELETE FROM {table} WHERE recovery_id IN (SELECT recovery_id FROM sync_recovery_archives WHERE local_root_uuid=?1)"
            ),
            [root.as_str()],
        )
        .map_err(storage)?;
    }
    c.execute(
        "DELETE FROM sync_recovery_archives WHERE local_root_uuid=?1",
        [root.as_str()],
    )
    .map_err(storage)?;
    Ok(())
}

fn is_sync_protocol_state(json: &str) -> Result<bool, Error> {
    let envelope: serde_json::Value = serde_json::from_str(json).map_err(storage)?;
    let scope = envelope
        .pointer("/key/namespace/scope")
        .and_then(serde_json::Value::as_str);
    let schema = envelope
        .pointer("/payload/schema")
        .and_then(serde_json::Value::as_str);
    Ok(
        matches!(scope, Some("bound" | "local_unbound" | "legacy_evidence"))
            || schema.is_some_and(|schema| schema.starts_with("echo.sync.")),
    )
}
pub(super) fn status(
    c: &Connection,
    root: LibraryRootId,
) -> Result<LibraryProjectionStatus, Error> {
    let watermark: Option<(u64,Option<String>)> = c.query_row("SELECT commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1",[root.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
    let mut result = LibraryProjectionStatus::default();
    if let Some((seq, head)) = watermark {
        result.applied_library_commit_seq = seq;
        result.applied_head_digest = head;
    }
    let mut statement = c.prepare("SELECT row_kind,object_key,unresolved_reason FROM library_recovery_records WHERE library_root_uuid=?1 AND unresolved_reason IS NOT NULL ORDER BY object_key").map_err(storage)?;
    let rows = statement
        .query_map([root.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(storage)?;
    for row in rows {
        let (kind, key, reason) = row.map_err(storage)?;
        let target = if kind == "record" {
            ProjectionTarget::Record {
                object: serde_json::from_str(&key).map_err(storage)?,
            }
        } else {
            serde_json::from_str(&key).map_err(storage)?
        };
        result.unresolved.push(UnresolvedProjectionRecord {
            target,
            reason: serde_json::from_str(&reason).map_err(storage)?,
        });
    }
    Ok(result)
}
