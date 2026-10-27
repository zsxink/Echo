//! Install one authorized replacement event for a reconciled historical intent.
//!
//! The adoption record itself is committed to the directory before this runs.
//! A proven (accepted/terminal) verdict installs nothing: the old event stays
//! read-only evidence. An explicitly kept intent installs exactly one sendable
//! row under the new author, carrying the original body, object identity and
//! frozen change time. The replacement identity is derived from the adoption
//! key, so re-running an interrupted transition can never create a second event.
use super::{
    Connection, DurableStateChange, Error, OptionalExtension, Reason, StateTarget,
    SyncRecoveryContext, Value, params, schema, storage,
};
use crate::domain::library::durability::{MutationOperation, adoption};
use crate::domain::sync::ObjectKey;

/// The stored object kind of a historical event, back into its typed identity.
fn object_key(kind: &str, id: uuid::Uuid) -> Result<ObjectKey, Error> {
    use crate::domain::ids::{PlaylistId, PlaylistItemId, SongId};
    Ok(match kind {
        "song" => ObjectKey::Song(SongId::from_uuid(id)),
        "playlist" => ObjectKey::Playlist(PlaylistId::from_uuid(id)),
        "playlist-item" => ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(id)),
        "library-root" => ObjectKey::LibraryRoot(id),
        "favorite" => ObjectKey::Favorite(SongId::from_uuid(id)),
        "override" => ObjectKey::Override(SongId::from_uuid(id)),
        "play-stats" => ObjectKey::PlayStats(SongId::from_uuid(id)),
        _ => return Err(Error::conflict("unknown adoption object kind")),
    })
}

pub(super) fn project(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
) -> Result<Option<Reason>, Error> {
    let Some(key) = state.key.as_ref() else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    if !schema(state, "echo.sync.adoption.v1")
        || !matches!(key.target, StateTarget::Adoption { .. })
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let record = adoption(&state.payload)?;
    record.validate(key)?;
    if !record.verdict.replaces_intent() {
        // Acceptance (or a terminal rejection) is proven under this binding.
        // Nothing may be applied or uploaded in the old event's name.
        return Ok(None);
    }
    let Some(new_id) = record.new_mutation_id else {
        // D5: an old author's unconfirmed play-stats bucket is deliberately not
        // adopted. The record documents the decision and the old event stays
        // pending under its original author, so there is nothing to install.
        return Ok(None);
    };
    if record.new_author != context.local_author {
        // Only the device the explicit transition named may send its event.
        return Ok(Some(Reason::RequiresStateRecovery));
    }
    install(c, context, &record, new_id)
}

fn install(
    c: &Connection,
    context: SyncRecoveryContext,
    record: &crate::domain::library::durability::AdoptionState,
    new_id: crate::domain::sync::MutationId,
) -> Result<Option<Reason>, Error> {
    let resources: Vec<crate::domain::sync::SharedResourceManifest> =
        serde_json::from_str(&record.original_resources_body).map_err(storage)?;
    crate::infrastructure::sqlite::sync::manifests::validate(&resources)?;
    let operation = match record.old_operation {
        MutationOperation::Upsert => "upsert",
        MutationOperation::Delete => "delete",
    };
    let object = record.old_object_uuid.to_string();
    // The new event re-declares the original bytes; its digest is recomputed
    // over exactly those bytes so the host can dedupe by mutation identity.
    let digest = crate::infrastructure::sqlite::sync::enqueue::snapshot_digest(
        &record.old_object_type,
        &object,
        &record.original_body,
        operation,
        &record.original_resources_body,
    );
    match reuse_existing(c, context, record, new_id, operation, &object, &digest)? {
        ExistingMutation::Reused => return Ok(None),
        ExistingMutation::Conflict => return Ok(Some(Reason::AmbiguousMutationVersion)),
        ExistingMutation::Missing => {}
    }
    insert_new(
        c, context, record, new_id, operation, &object, &digest, &resources,
    )
}

enum ExistingMutation {
    Missing,
    Reused,
    Conflict,
}

fn reuse_existing(
    c: &Connection,
    context: SyncRecoveryContext,
    record: &crate::domain::library::durability::AdoptionState,
    new_id: crate::domain::sync::MutationId,
    operation: &str,
    object: &str,
    digest: &str,
) -> Result<ExistingMutation, Error> {
    let expected = serde_json::json!({"author":record.new_author.as_uuid().to_string(),"root":context.root.to_string(),"kind":record.old_object_type,"object":object,"operation":operation,"body":record.original_body,"resources":record.original_resources_body,"digest":digest,"time":record.old_frozen_at_ms});
    let existing: Option<String> = c
        .query_row(
            "SELECT json_object('author',origin_device_id,'root',local_root_uuid,'kind',object_type,'object',object_uuid,'operation',operation,'body',payload_json,'resources',resource_refs_json,'digest',payload_digest,'time',created_at) FROM sync_mutations WHERE mutation_id=?1",
            [new_id.as_uuid().to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    let Some(raw) = existing else {
        return Ok(ExistingMutation::Missing);
    };
    if serde_json::from_str::<Value>(&raw).map_err(storage)? != expected {
        return Ok(ExistingMutation::Conflict);
    }
    // A retried transition reuses the allocation the first attempt already
    // made; the cursor must never be rewound by the replay.
    let (sequence, revision): (i64, i64) = c
        .query_row(
            "SELECT origin_seq,local_revision FROM sync_mutations WHERE mutation_id=?1",
            [new_id.as_uuid().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(storage)?;
    advance(
        c,
        context,
        &record.new_author,
        &new_id.as_uuid().to_string(),
        sequence,
        revision,
        &record.old_object_type,
        object,
    )?;
    Ok(ExistingMutation::Reused)
}

#[allow(clippy::too_many_arguments)]
fn insert_new(
    c: &Connection,
    context: SyncRecoveryContext,
    record: &crate::domain::library::durability::AdoptionState,
    new_id: crate::domain::sync::MutationId,
    operation: &str,
    object: &str,
    digest: &str,
    resources: &[crate::domain::sync::SharedResourceManifest],
) -> Result<Option<Reason>, Error> {
    let payload: Value = serde_json::from_str(&record.original_body).map_err(storage)?;
    if let Some(reason) = super::objects::dependency(
        c,
        context,
        object_key(&record.old_object_type, record.old_object_uuid)?,
        &payload,
        record.old_operation == MutationOperation::Delete,
    )? {
        return Ok(Some(reason));
    }
    let cursor: Option<(i64, String)> = c
        .query_row(
            "SELECT origin_seq,last_mutation_id FROM sync_local_origin_cursors WHERE origin_device_id=?1 AND local_root_uuid=?2",
            params![record.new_author.as_uuid().to_string(), context.root.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let sequence = cursor.as_ref().map_or(Ok(1), |(seq, _)| {
        seq.checked_add(1)
            .ok_or_else(|| Error::conflict("sync origin sequence exhausted"))
    })?;
    let predecessor = cursor.map(|(_, mutation_id)| mutation_id);
    let occupied: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND origin_device_id=?2 AND origin_seq=?3)",
            params![context.root.to_string(), record.new_author.as_uuid().to_string(), sequence],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if occupied {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute(
        "INSERT INTO sync_local_highwater(object_type,object_uuid,local_revision) VALUES(?1,?2,1) ON CONFLICT(object_type,object_uuid) DO UPDATE SET local_revision=local_revision+1",
        params![record.old_object_type, object],
    )
    .map_err(storage)?;
    let revision: i64 = c
        .query_row(
            "SELECT local_revision FROM sync_local_highwater WHERE object_type=?1 AND object_uuid=?2",
            params![record.old_object_type, object],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let base: Option<i64> = c
        .query_row(
            "SELECT object_version FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2 AND is_tombstone=0 UNION ALL SELECT object_version FROM archived_sync_object_versions WHERE context_root=?3 AND object_type=?1 AND object_uuid=?2 AND is_tombstone=0 LIMIT 1",
            params![record.old_object_type, object, context.root.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    c.execute("INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,previous_mutation_id,object_type,object_uuid,operation,local_revision,base_object_version,payload_json,payload_digest,status,outcome_json,created_at,resource_refs_json,local_root_uuid) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',NULL,?12,?13,?14)",params![new_id.as_uuid().to_string(),record.new_author.as_uuid().to_string(),sequence,predecessor,record.old_object_type,object,operation,revision,base,record.original_body,digest,i64::try_from(record.old_frozen_at_ms).unwrap_or(i64::MAX),record.original_resources_body,context.root.to_string()]).map_err(storage)?;
    advance(
        c,
        context,
        &record.new_author,
        &new_id.as_uuid().to_string(),
        sequence,
        revision,
        &record.old_object_type,
        object,
    )?;
    // A newer pending event for the same object already owns its declaration;
    // only the effective (newest) intent may replace the upload manifest.
    let newer: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND object_type=?2 AND object_uuid=?3 AND status='pending' AND (local_revision>?4 OR (local_revision=?4 AND origin_seq>?5)))",
        params![context.root.to_string(), record.old_object_type, object, revision, sequence],
        |r| r.get(0),
    ).map_err(storage)?;
    if !newer {
        crate::infrastructure::sqlite::sync::manifests::replace(
            c,
            &record.old_object_type,
            object,
            resources,
        )?;
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn advance(
    c: &Connection,
    context: SyncRecoveryContext,
    author: &crate::domain::sync::DeviceId,
    mutation_id: &str,
    sequence: i64,
    revision: i64,
    kind: &str,
    object: &str,
) -> Result<(), Error> {
    c.execute("INSERT INTO sync_local_highwater(object_type,object_uuid,local_revision) VALUES(?1,?2,?3) ON CONFLICT(object_type,object_uuid) DO UPDATE SET local_revision=MAX(local_revision,excluded.local_revision)",params![kind,object,revision]).map_err(storage)?;
    c.execute("INSERT INTO sync_local_origin_cursors(local_root_uuid,origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3,?4) ON CONFLICT(local_root_uuid,origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id WHERE origin_seq<excluded.origin_seq",params![context.root.to_string(),author.as_uuid().to_string(),sequence,mutation_id]).map_err(storage)?;
    Ok(())
}
