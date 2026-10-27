//! Explicit boundary mapping from retained SQL evidence into domain facts.
use super::{parse_uuid, text};
use crate::domain::library::PortableSerialize;
use crate::domain::{
    ids::Revision,
    library::durability::{
        DurableStateChange, FrozenMutation, MutationDisposition, MutationOperation, MutationStatus,
        StateKey, StateNamespace, StateTarget,
    },
    sync::{
        AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey, ObjectVersion, SharedResourceManifest, SyncBinding,
    },
};
use crate::error::Error;
use crate::infrastructure::sqlite::support::storage;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

pub(super) fn object(payload: &Value) -> Result<ObjectKey, Error> {
    let id = parse_uuid(payload, "object_uuid")?;
    Ok(match text(payload, "object_type")? {
        "song" => ObjectKey::Song(crate::domain::ids::SongId::from_uuid(id)),
        "playlist" => ObjectKey::Playlist(crate::domain::ids::PlaylistId::from_uuid(id)),
        "playlist-item" => {
            ObjectKey::PlaylistItem(crate::domain::ids::PlaylistItemId::from_uuid(id))
        }
        "library-root" => ObjectKey::LibraryRoot(id),
        "favorite" => ObjectKey::Favorite(crate::domain::ids::SongId::from_uuid(id)),
        "override" => ObjectKey::Override(crate::domain::ids::SongId::from_uuid(id)),
        "play-stats" => ObjectKey::PlayStats(crate::domain::ids::SongId::from_uuid(id)),
        _ => return Err(Error::conflict("unknown typed recovery object")),
    })
}
fn number(payload: &Value, field: &str) -> Result<u64, Error> {
    payload
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::conflict("invalid numeric recovery fact"))
}
pub(super) fn binding(payload: &Value) -> Result<SyncBinding, Error> {
    Ok(SyncBinding {
        library_id: LibraryId::from_uuid(parse_uuid(payload, "library")?),
        authority_id: AuthorityId::from_uuid(parse_uuid(payload, "authority")?),
        epoch: Epoch::from_u64(number(payload, "epoch")?),
    })
}
pub(super) fn context_namespace(
    connection: &Connection,
    root: &str,
    active: bool,
) -> Result<Option<StateNamespace>, Error> {
    let query = if active {
        "SELECT json_object('library',library_id,'authority',authority_id,'epoch',epoch) FROM sync_binding WHERE singleton=1 AND local_root_uuid=?1"
    } else {
        "SELECT json_object('library',library_id,'authority',authority_id,'epoch',epoch) FROM archived_sync_binding WHERE context_root=?1 AND local_root_uuid=?1"
    };
    let raw: Option<String> = connection
        .query_row(query, [root], |row| row.get(0))
        .optional()
        .map_err(storage)?;
    raw.map(|raw| {
        Ok(StateNamespace::Bound {
            binding: binding(&serde_json::from_str(&raw).map_err(storage)?)?,
        })
    })
    .transpose()
}
pub(super) fn mutation(
    payload: &Value,
    namespace: StateNamespace,
) -> Result<[DurableStateChange; 2], Error> {
    let author = DeviceId::from_uuid(parse_uuid(payload, "author")?);
    let mutation_id = MutationId::from_uuid(parse_uuid(payload, "mutation_id")?);
    // The queue persists the domain manifests directly, unlike LAN commit
    // JSON, whose wire resources use content_hash and string paths.
    let resources: Vec<SharedResourceManifest> =
        serde_json::from_value(payload["resources"].clone()).map_err(storage)?;
    super::super::manifests::validate(&resources)?;
    let candidate = MutationCandidate {
        mutation_id,
        origin_device_id: author,
        origin_seq: number(payload, "sequence")?,
        previous_mutation_id: payload
            .get("predecessor")
            .filter(|v| !v.is_null())
            .map(|_| parse_uuid(payload, "predecessor").map(MutationId::from_uuid))
            .transpose()?,
        object: object(payload)?,
        base_object_version: payload
            .get("base_version")
            .filter(|v| !v.is_null())
            .map(|_| {
                number(payload, "base_version")
                    .map(|v| ObjectVersion::from_commit_seq(CommitSeq::from_u64(v)))
            })
            .transpose()?,
        payload: payload["payload"].clone(),
        resource_refs: resources,
    };
    let frozen = FrozenMutation {
        candidate,
        operation: match text(payload, "operation")? {
            "upsert" => MutationOperation::Upsert,
            "delete" => MutationOperation::Delete,
            _ => return Err(Error::conflict("invalid mutation operation")),
        },
        local_revision: Revision::from_u64(number(payload, "revision")?),
        frozen_at_ms: number(payload, "changed_at_ms")?,
        original_body: text(payload, "original_body")?.to_owned(),
        original_resources_body: text(payload, "original_resources_body")?.to_owned(),
        payload_digest: text(payload, "payload_digest")?.to_owned(),
    };
    let body_digest = frozen.content_hash()?;
    let disposition = MutationDisposition {
        author,
        mutation_id,
        frozen_body_digest: body_digest.clone(),
        status: match text(payload, "status")? {
            "pending" => MutationStatus::Pending,
            "confirmed" => MutationStatus::Confirmed,
            "rejected" => MutationStatus::Rejected,
            _ => return Err(Error::conflict("invalid mutation status")),
        },
        outcome: payload
            .get("outcome")
            .filter(|value| !value.is_null())
            .cloned(),
    };
    Ok([
        DurableStateChange::scoped(
            StateKey {
                namespace: namespace.clone(),
                target: StateTarget::Mutation {
                    author,
                    mutation_id,
                    body_digest: body_digest.clone(),
                },
            },
            json!({"schema":"echo.sync.frozen-mutation.v2","frozen":frozen}),
        )?,
        DurableStateChange::scoped(
            StateKey {
                namespace,
                target: StateTarget::MutationDisposition {
                    author,
                    mutation_id,
                    body_digest,
                },
            },
            json!({"schema":"echo.sync.mutation-disposition.v2","disposition":disposition}),
        )?,
    ])
}
pub(super) fn target(family: &str, payload: &Value) -> Result<StateTarget, Error> {
    Ok(match family {
        "object" => StateTarget::Confirmed {
            object: object(payload)?,
        },
        "song-alias" => StateTarget::SongAlias {
            alias: crate::domain::ids::SongId::from_uuid(parse_uuid(payload, "alias")?),
        },
        "member-alias" => StateTarget::MemberAlias {
            playlist: crate::domain::ids::PlaylistId::from_uuid(parse_uuid(payload, "playlist")?),
            alias: crate::domain::ids::PlaylistItemId::from_uuid(parse_uuid(payload, "alias")?),
        },
        "commit" => StateTarget::Ledger {
            author: DeviceId::from_uuid(parse_uuid(payload, "author")?),
            mutation_id: MutationId::from_uuid(parse_uuid(payload, "mutation_id")?),
        },
        "receipt" => StateTarget::Receipt {
            author: DeviceId::from_uuid(parse_uuid(payload, "author")?),
            mutation_id: MutationId::from_uuid(parse_uuid(payload, "mutation_id")?),
        },
        "origin" => StateTarget::OriginCursor {
            author: DeviceId::from_uuid(parse_uuid(payload, "author")?),
        },
        "peer" => StateTarget::PeerCursor {
            peer: DeviceId::from_uuid(parse_uuid(payload, "peer")?),
        },
        "conflict" => StateTarget::Conflict {
            conflict_id: parse_uuid(payload, "id")?,
        },
        _ => return Err(Error::conflict("unknown recovery fact family")),
    })
}
