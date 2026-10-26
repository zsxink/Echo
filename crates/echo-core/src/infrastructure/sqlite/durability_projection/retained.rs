//! Stable typed storage, dependency resolution and path-free recovery reporting.
mod artist_cover;
mod dependencies;
mod local_allocation;
use super::super::support::storage;
use crate::domain::{
    ids::LibraryRootId,
    library::{
        durability::{
            DurableResource, DurableStateChange, LibraryProjectionStatus, LibraryRecoveryRecords,
            PlaybackEvent, ProjectionTarget, ProjectionUnresolvedReason as Reason, StateKind,
            UnresolvedProjectionRecord,
        },
        PortableRecord, PortableRecordEnvelope,
    },
};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension};

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
    let pending = (kind == "record")
        .then(|| serde_json::to_string(&Reason::MissingSyncDependency))
        .transpose()
        .map_err(storage)?;
    c.execute("INSERT INTO library_recovery_records(library_root_uuid,object_key,envelope_json,row_kind,unresolved_reason) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(library_root_uuid,object_key) DO UPDATE SET envelope_json=excluded.envelope_json,row_kind=excluded.row_kind,unresolved_reason=excluded.unresolved_reason",params![root.to_string(),key,json,kind,pending]).map_err(storage)?;
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
    c.execute("UPDATE library_recovery_records SET unresolved_reason=?3 WHERE library_root_uuid=?1 AND object_key=?2",params![root.to_string(),key,value]).map_err(storage)?;
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
    let retained = read(c, root)?;
    let mut statement = c.prepare(
        "SELECT object_key FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='record' AND unresolved_reason IS NOT NULL",
    ).map_err(storage)?;
    let pending = statement
        .query_map([root.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .map_err(storage)?;
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
        reason(c, root, &key, unresolved)?;
    }
    for state in &retained.state {
        let unresolved = if state.kind == StateKind::ArtistCover {
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
            local_allocation::restore(c, root, library, state, &retained.state)?
        };
        reason(c, root, &state_key(state)?, unresolved)?;
    }
    Ok(())
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
