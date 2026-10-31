//! Install exact frozen event bytes; never enqueue or re-author a recovered edit.
use super::{
    params, schema, storage, Connection, DurableStateChange, Error, OptionalExtension, Reason,
    StateTarget, SyncRecoveryContext, Value,
};
use crate::domain::library::durability::{
    FrozenMutation, MutationDisposition, MutationOperation, MutationStatus,
};

pub(super) fn project(
    c: &Connection,
    context: SyncRecoveryContext,
    state: &DurableStateChange,
    states: &[&DurableStateChange],
) -> Result<Option<Reason>, Error> {
    let key = state
        .key
        .as_ref()
        .ok_or_else(|| Error::conflict("mutation has no key"))?;
    let (author, id, digest) = match &key.target {
        StateTarget::Mutation {
            author,
            mutation_id,
            body_digest,
        }
        | StateTarget::MutationDisposition {
            author,
            mutation_id,
            body_digest,
        } => (*author, *mutation_id, body_digest),
        _ => return Ok(Some(Reason::InvalidStateFact)),
    };
    if author != context.local_author {
        return Ok(Some(Reason::RequiresStateRecovery));
    }
    let Some(frozen_state)=states.iter().find(|s|matches!(s.key.as_ref().map(|k|&k.target),Some(StateTarget::Mutation {author:a,mutation_id:i,body_digest:d}) if *a==author && *i==id && d==digest)) else {return Ok(Some(Reason::MissingSyncDependency));};
    let Some(disposition_state)=states.iter().find(|s|matches!(s.key.as_ref().map(|k|&k.target),Some(StateTarget::MutationDisposition {author:a,mutation_id:i,body_digest:d}) if *a==author && *i==id && d==digest)) else {return Ok(Some(Reason::MissingSyncDependency));};
    if !schema(frozen_state, "echo.sync.frozen-mutation.v2")
        || !schema(disposition_state, "echo.sync.mutation-disposition.v2")
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    frozen_state.validate()?;
    disposition_state.validate()?;
    let frozen: FrozenMutation =
        serde_json::from_value(frozen_state.payload["frozen"].clone()).map_err(storage)?;
    let disposition: MutationDisposition =
        serde_json::from_value(disposition_state.payload["disposition"].clone())
            .map_err(storage)?;
    let candidate = &frozen.candidate;
    let (kind, object) =
        crate::infrastructure::sqlite::sync::apply::object_identity(candidate.object);
    let operation = match frozen.operation {
        MutationOperation::Upsert => "upsert",
        MutationOperation::Delete => "delete",
    };
    let predecessor = candidate
        .previous_mutation_id
        .map(|id| id.as_uuid().to_string());
    let expected = serde_json::json!({"author":author.as_uuid().to_string(),"root":context.root.to_string(),"sequence":candidate.origin_seq,"predecessor":predecessor,"kind":kind,"object":object,"operation":operation,"revision":frozen.local_revision.as_u64(),"base":candidate.base_object_version.map(|v|v.commit_seq().as_u64()),"body":frozen.original_body,"resources":frozen.original_resources_body,"digest":frozen.payload_digest,"time":frozen.frozen_at_ms});
    let existing:Option<String>=c.query_row("SELECT json_object('author',origin_device_id,'root',local_root_uuid,'sequence',origin_seq,'predecessor',previous_mutation_id,'kind',object_type,'object',object_uuid,'operation',operation,'revision',local_revision,'base',base_object_version,'body',payload_json,'resources',resource_refs_json,'digest',payload_digest,'time',created_at) FROM sync_mutations WHERE mutation_id=?1",[id.as_uuid().to_string()],|r|r.get(0)).optional().map_err(storage)?;
    if let Some(raw) = existing {
        if serde_json::from_str::<Value>(&raw).map_err(storage)? != expected {
            return Ok(Some(Reason::AmbiguousMutationVersion));
        }
    } else {
        // A frozen declaration is not proof that its upload bytes/publication
        // dependencies survived. A later specialized resource consumer must
        // verify those before making a recovered resource event sendable.
        if !candidate.resource_refs.is_empty() {
            return Ok(Some(Reason::RequiresStateRecovery));
        }
        // Rebase versions share an event ID. Without an active-version seal, no
        // directory ordering, wall clock or lexical digest may pick the sender.
        if states.iter().filter(|s|matches!(s.key.as_ref().map(|k|&k.target),Some(StateTarget::Mutation {author:a,mutation_id:i,..}) if *a==author && *i==id)).count()!=1 {return Ok(Some(Reason::AmbiguousMutationVersion));}
        if let Some(reason) = super::objects::dependency(
            c,
            context,
            candidate.object,
            &candidate.payload,
            frozen.operation == MutationOperation::Delete,
        )? {
            return Ok(Some(reason));
        }
        if !predecessor_known(c, context, &frozen)? {
            return Ok(Some(Reason::MissingSyncDependency));
        }
        if let Some(base) = candidate.base_object_version {
            let known:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2 AND object_version=?3)",params![kind,object,base.commit_seq().as_u64()],|r|r.get(0)).map_err(storage)?;
            if !known {
                return Ok(Some(Reason::MissingSyncDependency));
            }
        }
        let occupied:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND origin_device_id=?2 AND origin_seq=?3)",params![context.root.to_string(),author.as_uuid().to_string(),candidate.origin_seq],|r|r.get(0)).map_err(storage)?;
        if occupied {
            return Ok(Some(Reason::IdentityConflict));
        }
        c.execute("INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,previous_mutation_id,object_type,object_uuid,operation,local_revision,base_object_version,payload_json,payload_digest,status,outcome_json,created_at,resource_refs_json,local_root_uuid) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',NULL,?12,?13,?14)",params![id.as_uuid().to_string(),author.as_uuid().to_string(),candidate.origin_seq,predecessor,kind,object,operation,frozen.local_revision.as_u64(),candidate.base_object_version.map(|v|v.commit_seq().as_u64()),frozen.original_body,frozen.payload_digest,frozen.frozen_at_ms,frozen.original_resources_body,context.root.to_string()]).map_err(storage)?;
    }
    finish(c, context, &frozen, &disposition)
}
fn finish(
    c: &Connection,
    context: SyncRecoveryContext,
    frozen: &FrozenMutation,
    disposition: &MutationDisposition,
) -> Result<Option<Reason>, Error> {
    let candidate = &frozen.candidate;
    let author = candidate.origin_device_id;
    let id = candidate.mutation_id;
    let (kind, object) =
        crate::infrastructure::sqlite::sync::apply::object_identity(candidate.object);
    let disposition_status = match disposition.status {
        MutationStatus::Pending => "pending",
        MutationStatus::Confirmed => "confirmed",
        MutationStatus::Rejected => "rejected",
    };
    let outcome = disposition
        .outcome
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(storage)?;
    let old: (String, Option<String>) = c
        .query_row(
            "SELECT status,outcome_json FROM sync_mutations WHERE mutation_id=?1",
            [id.as_uuid().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(storage)?;
    if old.0 != "pending" && disposition_status != old.0 && disposition_status != "pending" {
        return Ok(Some(Reason::IdentityConflict));
    }
    if old.0 == "pending" {
        c.execute(
            "UPDATE sync_mutations SET status=?2,outcome_json=?3 WHERE mutation_id=?1",
            params![id.as_uuid().to_string(), disposition_status, outcome],
        )
        .map_err(storage)?;
    } else if disposition_status == old.0 && old.1 != outcome {
        return Ok(Some(Reason::IdentityConflict));
    }
    if disposition.status == MutationStatus::Pending && old.0 == "pending" {
        let newer:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND object_type=?2 AND object_uuid=?3 AND status='pending' AND (local_revision>?4 OR (local_revision=?4 AND origin_seq>?5)))",params![context.root.to_string(),kind,object,frozen.local_revision.as_u64(),candidate.origin_seq],|r|r.get(0)).map_err(storage)?;
        if !newer {
            crate::infrastructure::sqlite::sync::manifests::replace(
                c,
                kind,
                &object,
                &candidate.resource_refs,
            )?;
        }
    } else if old.0 == "pending" && disposition_status != "pending" {
        // Confirmed projection ran while this queue entry was still effective.
        // Re-select after the terminal transition, including any remaining
        // pending entry in this root, in the same recovery transaction.
        super::objects::restore_resources(c, context, kind, &object)?;
    }
    c.execute("INSERT INTO sync_local_highwater(object_type,object_uuid,local_revision) VALUES(?1,?2,?3) ON CONFLICT(object_type,object_uuid) DO UPDATE SET local_revision=MAX(local_revision,excluded.local_revision)",params![kind,object,frozen.local_revision.as_u64()]).map_err(storage)?;
    c.execute("INSERT INTO sync_local_origin_cursors(local_root_uuid,origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3,?4) ON CONFLICT(local_root_uuid,origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id WHERE origin_seq<excluded.origin_seq",params![context.root.to_string(),author.as_uuid().to_string(),candidate.origin_seq,id.as_uuid().to_string()]).map_err(storage)?;
    Ok(None)
}
fn predecessor_known(
    c: &Connection,
    context: SyncRecoveryContext,
    frozen: &FrozenMutation,
) -> Result<bool, Error> {
    let candidate = &frozen.candidate;
    if candidate.origin_seq == 1 {
        return Ok(candidate.previous_mutation_id.is_none());
    }
    let Some(previous) = candidate.previous_mutation_id else {
        return Ok(false);
    };
    c.query_row("SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE mutation_id=?1 AND local_root_uuid=?2 AND origin_device_id=?3 AND origin_seq=?4) OR EXISTS(SELECT 1 FROM sync_origin_cursors WHERE origin_device_id=?3 AND origin_seq=?4 AND last_mutation_id=?1)",params![previous.as_uuid().to_string(),context.root.to_string(),context.local_author.as_uuid().to_string(),candidate.origin_seq-1],|r|r.get(0)).map_err(storage)
}
