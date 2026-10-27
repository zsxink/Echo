//! Restore facts whose complete dependencies are proven by the anchored ledger.
use super::{
    Connection, DurableStateChange, Error, OptionalExtension, Reason, StateTarget,
    SyncRecoveryContext, Value, ledger, number, params, schema, storage, text,
};
use crate::application::ports::StagedConfirmation;
use crate::domain::sync::MutationId;

type ConflictRow = (String, String, u64, u64, String, String, Option<u64>);

pub(super) fn project(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    match state.key.as_ref().map(|key| &key.target) {
        Some(StateTarget::Receipt { .. }) => receipt(c, context, state, ledger),
        Some(StateTarget::SongAlias { .. }) => song_alias(c, context, state, ledger),
        Some(StateTarget::MemberAlias { .. }) => member_alias(c, context, state, ledger),
        Some(StateTarget::OriginCursor { .. }) => origin_cursor(c, context, state, ledger),
        Some(StateTarget::Conflict { .. }) => conflict(c, context, state, ledger),
        // Historical publications/peer cursors lack complete authenticated
        // dependencies; no current binding can supply those missing facts.
        _ => Ok(Some(Reason::RequiresStateRecovery)),
    }
}
fn receipt(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    let StateTarget::Receipt {
        author,
        mutation_id,
    } = &state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("missing related key"))?
        .target
    else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let p = &state.payload;

    if !schema(state, "echo.sync.receipt.v1") && !schema(state, "echo.sync.receipt.v2") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let Some(seq) = p.get("sequence").and_then(Value::as_u64) else {
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    let Some(commit) = ledger.commits.get(&seq) else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    if commit.origin_device_id != *author || commit.mutation_id != *mutation_id {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let (Some(raw), Some(received)) = (
        p.get("original_outcome").and_then(Value::as_str),
        p.get("received_at").and_then(Value::as_u64),
    ) else {
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    if serde_json::from_str::<Value>(raw).map_err(storage)? != p["outcome"] {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let staged: StagedConfirmation = serde_json::from_str(raw).map_err(storage)?;
    if staged.mutation_id != *mutation_id
        || staged.checkpoint.is_none_or(|point| {
            point.binding != context.verified_checkpoint.binding
                || point.sequence != commit.sequence
                || point.digest != commit.digest
        })
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let old:Option<(String,String,Option<u64>)>=c.query_row("SELECT payload_digest,outcome_json,commit_seq FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",params![author.as_uuid().to_string(),mutation_id.as_uuid().to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
    let digest = text(p, "payload_digest")?;
    if old
        .as_ref()
        .is_some_and(|old| old != &(digest.to_owned(), raw.to_owned(), Some(seq)))
    {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute("INSERT OR IGNORE INTO sync_inbox(origin_device_id,mutation_id,payload_digest,outcome_json,commit_seq,received_at) VALUES(?1,?2,?3,?4,?5,?6)",params![author.as_uuid().to_string(),mutation_id.as_uuid().to_string(),digest,raw,seq,received]).map_err(storage)?;
    Ok(None)
}

fn song_alias(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    let StateTarget::SongAlias { alias } = &state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("missing related key"))?
        .target
    else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let p = &state.payload;

    if !schema(state, "echo.sync.song-alias.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let Some(commit) = ledger.commits.get(&number(p, "sequence")?) else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    let canonical = text(p, "canonical")?;
    let raw: Value = serde_json::from_str(&commit.canonical_json).map_err(storage)?;
    let proof = raw["aliases"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "song"
                && item.pointer("/alias/alias").and_then(Value::as_str) == Some(&alias.to_string())
                && item.pointer("/alias/canonical").and_then(Value::as_str) == Some(canonical)
        })
    });
    if !proof {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let owner: Option<String> = c
        .query_row(
            "SELECT library_root_uuid FROM songs WHERE uuid=?1",
            [canonical],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if owner.as_deref() != Some(&context.root.to_string()) {
        return Ok(Some(if owner.is_none() {
            Reason::MissingSong
        } else {
            Reason::IdentityConflict
        }));
    }
    let old: Option<String> = c
        .query_row(
            "SELECT canonical_song_uuid FROM sync_song_aliases WHERE alias_song_uuid=?1",
            [alias.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if old.as_ref().is_some_and(|old| old != canonical) {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute("INSERT OR IGNORE INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) VALUES(?1,?2,?3)",params![alias.to_string(),canonical,commit.sequence.as_u64()]).map_err(storage)?;
    Ok(None)
}

fn member_alias(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    let StateTarget::MemberAlias { playlist, alias } = &state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("missing related key"))?
        .target
    else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let p = &state.payload;

    if !schema(state, "echo.sync.member-alias.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let Some(commit) = ledger.commits.get(&number(p, "sequence")?) else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    let canonical = text(p, "canonical")?;
    let raw: Value = serde_json::from_str(&commit.canonical_json).map_err(storage)?;
    let proof = raw["aliases"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "playlist_member"
                && item.pointer("/alias/playlist_id").and_then(Value::as_str)
                    == Some(&playlist.to_string())
                && item.pointer("/alias/alias").and_then(Value::as_str) == Some(&alias.to_string())
                && item.pointer("/alias/canonical").and_then(Value::as_str) == Some(canonical)
        })
    });
    if !proof {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM playlist_songs s JOIN playlists p ON p.uuid=s.playlist_uuid WHERE s.member_uuid=?1 AND s.playlist_uuid=?2 AND p.library_root_uuid=?3)",params![canonical,playlist.to_string(),context.root.to_string()],|r|r.get(0)).map_err(storage)?;
    if !exists {
        return Ok(Some(Reason::MissingSyncDependency));
    }
    let old:Option<String>=c.query_row("SELECT canonical_member_uuid FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2",params![playlist.to_string(),alias.to_string()],|r|r.get(0)).optional().map_err(storage)?;
    if old.as_ref().is_some_and(|old| old != canonical) {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute("INSERT OR IGNORE INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)",params![playlist.to_string(),alias.to_string(),canonical,commit.sequence.as_u64()]).map_err(storage)?;
    Ok(None)
}

fn origin_cursor(
    c: &Connection,
    _context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    let StateTarget::OriginCursor { author } = &state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("missing related key"))?
        .target
    else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let p = &state.payload;

    if !schema(state, "echo.sync.origin-cursor.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let id: MutationId = serde_json::from_value(p["mutation_id"].clone()).map_err(storage)?;
    let sequence = number(p, "sequence")?;
    let proof = ledger.commits.values().any(|commit| {
        commit.origin_device_id == *author
            && commit.mutation_id == id
            && serde_json::from_str::<Value>(&commit.canonical_json).is_ok_and(|body| {
                body.pointer("/mutation/origin_seq").and_then(Value::as_u64) == Some(sequence)
            })
    });
    if !proof {
        return Ok(Some(Reason::MissingSyncDependency));
    }
    let old: Option<(u64, Option<String>)> = c
        .query_row(
            "SELECT origin_seq,last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1",
            [author.as_uuid().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    if old.as_ref().is_some_and(|(seq, last)| {
        *seq == sequence && last.as_deref() != Some(&id.as_uuid().to_string())
    }) {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute("INSERT INTO sync_origin_cursors(origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id WHERE origin_seq<excluded.origin_seq",params![author.as_uuid().to_string(),sequence,id.as_uuid().to_string()]).map_err(storage)?;
    Ok(None)
}

fn conflict(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    ledger: &ledger::Verified,
) -> Result<Option<Reason>, Error> {
    if !schema(state, "echo.sync.conflict.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let p = &state.payload;
    let replaced = number(p, "replaced_version")?;
    let accepted = number(p, "accepted_version")?;
    let (Some(before), Some(after)) =
        (ledger.commits.get(&replaced), ledger.commits.get(&accepted))
    else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    let (kind, object) = crate::infrastructure::sqlite::sync::apply::object_identity(after.object);
    if before.object != after.object
        || kind != text(p, "object_type")?
        || object != text(p, "object_uuid")?
        || serde_json::from_str::<Value>(&before.payload_json).map_err(storage)? != p["replaced"]
        || serde_json::from_str::<Value>(&after.payload_json).map_err(storage)? != p["accepted"]
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    if let Some(reason) =
        super::objects::dependency(c, context, after.object, &p["accepted"], after.is_tombstone)?
    {
        return Ok(Some(reason));
    }
    let old:Option<ConflictRow>=c.query_row("SELECT object_type,object_uuid,replaced_version,accepted_version,replaced_payload_json,accepted_payload_json,resolved_at FROM sync_conflicts WHERE conflict_id=?1",[text(p,"id")?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(storage)?;
    if let Some((
        old_kind,
        old_object,
        old_before,
        old_after,
        old_replaced,
        old_accepted,
        resolved,
    )) = old
    {
        if old_kind != kind
            || old_object != object
            || old_before != replaced
            || old_after != accepted
            || serde_json::from_str::<Value>(&old_replaced).map_err(storage)? != p["replaced"]
            || serde_json::from_str::<Value>(&old_accepted).map_err(storage)? != p["accepted"]
        {
            return Ok(Some(Reason::IdentityConflict));
        }
        if resolved.is_some() {
            return Ok(None);
        }
    }
    let resolved = p
        .get("resolved_at")
        .filter(|v| !v.is_null())
        .map(|_| number(p, "resolved_at"))
        .transpose()?;
    c.execute("INSERT INTO sync_conflicts(conflict_id,object_type,object_uuid,replaced_version,accepted_version,replaced_payload_json,accepted_payload_json,resolved_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(conflict_id) DO UPDATE SET resolved_at=excluded.resolved_at",params![text(p,"id")?,kind,object,replaced,accepted,super::body(p,"replaced")?,super::body(p,"accepted")?,resolved]).map_err(storage)?;
    Ok(None)
}
