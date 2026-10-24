//! Restore local allocation fences without installing a sendable mutation.
#[cfg(test)]
mod tests;
use super::{storage, Reason};
use crate::domain::{
    ids::LibraryRootId,
    library::{
        durability::{DurableStateChange, FrozenMutation, StateNamespace, StateTarget},
        LibraryId,
    },
};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn restore(
    c: &Connection,
    root: LibraryRootId,
    library: Option<LibraryId>,
    state: &DurableStateChange,
    states: &[DurableStateChange],
) -> Result<Option<Reason>, Error> {
    let Some(key) = &state.key else {
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    let (author, bound) = match &key.namespace {
        StateNamespace::LocalUnbound { library_id, author } if Some(*library_id) == library => {
            (*author, false)
        }
        StateNamespace::Bound { binding } if library.is_some() => {
            let matches: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND local_root_uuid=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4)",
                params![root.to_string(), binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), binding.epoch.as_u64()],
                |row| row.get(0),
            ).map_err(storage)?;
            if !matches {
                return Ok(Some(Reason::RequiresStateRecovery));
            }
            let (StateTarget::Mutation { author, .. }
            | StateTarget::MutationDisposition { author, .. }) = key.target
            else {
                return Ok(Some(Reason::RequiresStateRecovery));
            };
            (author, true)
        }
        _ => return Ok(Some(Reason::RequiresStateRecovery)),
    };
    let local_author: String = c
        .query_row(
            "SELECT value FROM device_state WHERE key='device_id'",
            [],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if author.as_uuid().to_string() != local_author {
        return Ok(Some(Reason::RequiresStateRecovery));
    }
    let (mutation_id, body_digest) = match &key.target {
        StateTarget::Mutation {
            mutation_id,
            body_digest,
            ..
        }
        | StateTarget::MutationDisposition {
            mutation_id,
            body_digest,
            ..
        } => (*mutation_id, body_digest),
        _ => return Ok(Some(Reason::RequiresStateRecovery)),
    };
    state.validate()?;
    let frozen_target = StateTarget::Mutation {
        author,
        mutation_id,
        body_digest: body_digest.clone(),
    };
    let disposition_target = StateTarget::MutationDisposition {
        author,
        mutation_id,
        body_digest: body_digest.clone(),
    };
    let paired = |target: &StateTarget| {
        states.iter().find(|other| {
            other.key.as_ref().is_some_and(|other_key| {
                other_key.namespace == key.namespace && other_key.target == *target
            })
        })
    };
    let (Some(frozen_state), Some(disposition_state)) =
        (paired(&frozen_target), paired(&disposition_target))
    else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    if frozen_state.payload["schema"] != "echo.sync.frozen-mutation.v2"
        || disposition_state.payload["schema"] != "echo.sync.mutation-disposition.v2"
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    frozen_state.validate()?;
    disposition_state.validate()?;
    let frozen: FrozenMutation =
        serde_json::from_value(frozen_state.payload["frozen"].clone()).map_err(storage)?;
    let unresolved = restore_fences(c, root, &local_author, &frozen, states)?;
    Ok(unresolved.or_else(|| bound.then_some(Reason::RequiresStateRecovery)))
}

fn restore_fences(
    c: &Connection,
    root: LibraryRootId,
    local_author: &str,
    frozen: &FrozenMutation,
    states: &[DurableStateChange],
) -> Result<Option<Reason>, Error> {
    let candidate = &frozen.candidate;
    let author = candidate.origin_device_id;
    if !candidate.resource_refs.is_empty() {
        return Ok(Some(Reason::RequiresStateRecovery));
    }
    for other in states {
        let Some(other_key) = &other.key else {
            continue;
        };
        if !matches!(other_key.target, StateTarget::Mutation { author: other_author, .. } if other_author == author)
        {
            continue;
        }
        other.validate()?;
        let other: FrozenMutation =
            serde_json::from_value(other.payload["frozen"].clone()).map_err(storage)?;
        if (other.candidate.origin_seq == candidate.origin_seq
            && other.candidate.mutation_id != candidate.mutation_id)
            || (other.candidate.mutation_id == candidate.mutation_id && other != *frozen)
        {
            return Ok(Some(Reason::IdentityConflict));
        }
    }
    let sequence = candidate.origin_seq;
    let mutation = candidate.mutation_id.as_uuid().to_string();
    let old: Option<(u64, String)> = c.query_row(
        "SELECT origin_seq,last_mutation_id FROM sync_local_origin_cursors WHERE local_root_uuid=?1 AND origin_device_id=?2",
        params![root.to_string(),local_author], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional().map_err(storage)?;
    if old
        .as_ref()
        .is_some_and(|(seq, id)| *seq == sequence && id != &mutation)
    {
        return Ok(Some(Reason::IdentityConflict));
    }
    let predecessor_known = if sequence == 1 {
        candidate.previous_mutation_id.is_none()
    } else if let Some(previous) = candidate.previous_mutation_id {
        let previous = previous.as_uuid().to_string();
        old.as_ref().is_some_and(|(seq, id)| *seq == sequence - 1 && id == &previous)
            || states.iter().any(|other| {
                other.key.as_ref().is_some_and(|other_key| {
                    matches!(other_key.target, StateTarget::Mutation { author: other_author, .. } if other_author == author)
                }) && serde_json::from_value::<FrozenMutation>(other.payload["frozen"].clone())
                    .is_ok_and(|other| {
                        other.candidate.origin_seq == sequence - 1
                            && other.candidate.mutation_id.as_uuid().to_string() == previous
                    })
            })
    } else {
        false
    };
    if !predecessor_known {
        return Ok(Some(Reason::MissingSyncDependency));
    }
    let occupied: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?1 AND origin_device_id=?2 AND origin_seq=?3 AND mutation_id<>?4)",
        params![root.to_string(),local_author,sequence,mutation], |r| r.get(0),
    ).map_err(storage)?;
    if occupied {
        return Ok(Some(Reason::IdentityConflict));
    }
    let (kind, object) =
        crate::infrastructure::sqlite::sync::apply::object_identity(candidate.object);
    c.execute(
        "INSERT INTO sync_local_highwater(object_type,object_uuid,local_revision) VALUES(?1,?2,?3) ON CONFLICT(object_type,object_uuid) DO UPDATE SET local_revision=MAX(local_revision,excluded.local_revision)",
        params![kind,object,frozen.local_revision.as_u64()],
    ).map_err(storage)?;
    c.execute(
        "INSERT INTO sync_local_origin_cursors(local_root_uuid,origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3,?4) ON CONFLICT(local_root_uuid,origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id WHERE origin_seq<excluded.origin_seq",
        params![root.to_string(),local_author,sequence,mutation],
    ).map_err(storage)?;
    Ok(None)
}
