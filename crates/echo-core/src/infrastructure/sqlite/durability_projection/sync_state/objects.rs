//! Restore confirmed protocol baselines without overwriting effective user values.
use super::{
    body, ledger, number, params, schema, storage, Connection, DurableStateChange, Error,
    OptionalExtension, Reason, StateTarget, SyncRecoveryContext, Value,
};
use crate::domain::sync::ObjectKey;

pub(super) fn dependency(
    c: &Connection,
    context: SyncRecoveryContext,
    object: ObjectKey,
    payload: &Value,
    tombstone: bool,
) -> Result<Option<Reason>, Error> {
    let (kind, id) = crate::infrastructure::sqlite::sync::apply::object_identity(object);
    let table = match object {
        ObjectKey::Playlist(_) => "playlists",
        ObjectKey::PlaylistItem(_) => "playlist_songs",
        ObjectKey::LibraryRoot(_) => return Ok(None),
        _ => "songs",
    };
    // A final tombstone is a first-class fact even after its live row is gone.
    if tombstone {
        let sql = if matches!(object, ObjectKey::PlaylistItem(_)) {
            "SELECT p.library_root_uuid FROM playlist_songs s JOIN playlists p ON p.uuid=s.playlist_uuid WHERE s.member_uuid=?1".to_owned()
        } else {
            format!("SELECT library_root_uuid FROM {table} WHERE uuid=?1")
        };
        let owner: Option<String> = c
            .query_row(&sql, [id], |r| r.get(0))
            .optional()
            .map_err(storage)?;
        return Ok(owner
            .filter(|owner| owner != &context.root.to_string())
            .map(|_| Reason::IdentityConflict));
    }
    if matches!(object, ObjectKey::PlaylistItem(_)) {
        for (field, table, missing) in [
            ("playlist_uuid", "playlists", Reason::MissingPlaylist),
            ("song_uuid", "songs", Reason::MissingSong),
        ] {
            let Some(parent) = payload.get(field).and_then(Value::as_str) else {
                return Ok(Some(Reason::InvalidStateFact));
            };
            let owner: Option<String> = c
                .query_row(
                    &format!("SELECT library_root_uuid FROM {table} WHERE uuid=?1"),
                    [parent],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            match owner {
                None => return Ok(Some(missing)),
                Some(owner) if owner != context.root.to_string() => {
                    return Ok(Some(Reason::IdentityConflict))
                }
                _ => {}
            }
        }
        return Ok(None);
    }
    let owner: Option<String> = c
        .query_row(
            &format!("SELECT library_root_uuid FROM {table} WHERE uuid=?1"),
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    Ok(match owner {
        Some(owner) if owner != context.root.to_string() => Some(Reason::IdentityConflict),
        None => Some(if kind == "playlist" {
            Reason::MissingPlaylist
        } else {
            Reason::MissingSong
        }),
        _ => None,
    })
}

pub(super) fn confirmed(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    if !schema(state, "echo.sync.confirmed.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let StateTarget::Confirmed { object } = state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("confirmed state has no key"))?
        .target
    else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let p = &state.payload;
    let sequence = number(p, "sequence")?;
    let version = number(p, "version")?;
    let Some(commit) = ledger.commits.get(&sequence) else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    let tombstone = p
        .get("tombstone")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::conflict("invalid confirmed tombstone flag"))?;
    let payload = body(p, "payload")?;
    if commit.object != object
        || commit.payload_json != payload
        || commit.is_tombstone != tombstone
        || version != sequence
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    if let Some(reason) = dependency(c, context, object, &p["payload"], tombstone)? {
        return Ok(Some(reason));
    }
    let (kind, id) = crate::infrastructure::sqlite::sync::apply::object_identity(object);
    let existing:Option<(u64,Option<String>,bool)>=c.query_row("SELECT object_version,payload_json,is_tombstone FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2",params![kind,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
    if let Some((old_version, old_body, old_deleted)) = existing {
        if old_version > version {
            return Ok(None);
        }
        if old_version == version {
            let same = old_body
                .as_ref()
                .map(|raw| serde_json::from_str::<Value>(raw))
                .transpose()
                .map_err(storage)?
                == Some(p["payload"].clone())
                && old_deleted == tombstone;
            return Ok((!same).then_some(Reason::IdentityConflict));
        }
        if old_deleted && !tombstone {
            return Ok(Some(Reason::TombstonedDependency));
        }
    }
    c.execute("INSERT INTO sync_object_versions(object_type,object_uuid,object_version,commit_seq,payload_json,is_tombstone,changed_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(object_type,object_uuid) DO UPDATE SET object_version=excluded.object_version,commit_seq=excluded.commit_seq,payload_json=excluded.payload_json,is_tombstone=excluded.is_tombstone,changed_at_ms=excluded.changed_at_ms",params![kind,id,version,sequence,payload,tombstone,number(p,"changed_at_ms")?]).map_err(storage)?;
    let resources =
        crate::infrastructure::sqlite::sync::manifests::from_commit_json(&commit.canonical_json)?;
    crate::infrastructure::sqlite::sync::manifests::replace(c, kind, &id, &resources)?;
    Ok(None)
}
