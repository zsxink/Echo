//! Freeze the fake's local allocation from the same durable prefix as SQLite.
use crate::domain::library::durability::{
    DurableStateChange, FrozenMutation, LibraryReplay, MutationDisposition, MutationOperation,
    MutationStatus, StateKey, StateNamespace, StateTarget,
};
use crate::domain::library::{LibraryId, PortableRecord, PortableSerialize, RecordKind};
use crate::domain::sync::{DeviceId, MutationCandidate, MutationId};
use crate::error::Error;
use serde_json::json;

pub(super) fn freeze(
    replay: &LibraryReplay,
    library_id: LibraryId,
    author: DeviceId,
    record: &PortableRecord,
    now: u64,
) -> Result<Vec<DurableStateChange>, Error> {
    freeze_after(replay, library_id, author, record, now, &[])
}

pub(super) fn freeze_after(
    replay: &LibraryReplay,
    library_id: LibraryId,
    author: DeviceId,
    record: &PortableRecord,
    now: u64,
    allocated: &[DurableStateChange],
) -> Result<Vec<DurableStateChange>, Error> {
    freeze_planned(replay, library_id, author, record, now, (allocated, &[]))
}

pub(super) fn freeze_planned(
    replay: &LibraryReplay,
    library_id: LibraryId,
    author: DeviceId,
    record: &PortableRecord,
    now: u64,
    planned: (
        &[DurableStateChange],
        &[crate::domain::library::PortableRecordEnvelope],
    ),
) -> Result<Vec<DurableStateChange>, Error> {
    let (allocated, records) = planned;
    let namespace = StateNamespace::LocalUnbound { library_id, author };
    let mut previous: Option<MutationCandidate> = None;
    for state in replay.states().chain(allocated) {
        if !state.key.as_ref().is_some_and(|key| {
            key.namespace == namespace
                && matches!(key.target, StateTarget::Mutation { author: other, .. } if other == author)
        }) {
            continue;
        }
        state.validate()?;
        let frozen: FrozenMutation =
            serde_json::from_value(state.payload["frozen"].clone()).map_err(encode)?;
        if previous
            .as_ref()
            .is_none_or(|old| old.origin_seq < frozen.candidate.origin_seq)
        {
            previous = Some(frozen.candidate);
        }
    }
    let sequence = previous.as_ref().map_or(Ok(1), |old| {
        old.origin_seq
            .checked_add(1)
            .ok_or_else(|| Error::conflict("origin sequence exhausted"))
    })?;
    let (kind, payload) = match record {
        PortableRecord::Playlist(record) => (
            "playlist",
            json!({
                "playlist_uuid": record.playlist_uuid, "display_name": record.display_name,
                "created_at": record.created_at, "cover_asset": record.cover_asset,
                "members": members(replay, record.playlist_uuid, records),
            }),
        ),
        PortableRecord::PlayStats(record) => {
            ("play-stats", serde_json::to_value(record).map_err(encode)?)
        }
        PortableRecord::Favorite(record) => (
            "favorite",
            json!({
                "song_uuid": record.song_uuid, "is_favorite": record.is_favorite,
                "favorited_at": record.favorited_at,
            }),
        ),
        PortableRecord::Tombstone(record) => (
            match record.deleted_kind {
                RecordKind::Playlist => "playlist",
                RecordKind::PlaylistItem => "playlist-item",
                _ => return Err(Error::conflict("unsupported fake delete")),
            },
            json!({"tombstone":true}),
        ),
        _ => return Err(Error::conflict("unsupported fake local edit")),
    };
    let id = record.object_uuid();
    let original_body = serde_json::to_string(&payload).map_err(encode)?;
    let mut digest = blake3::Hasher::new();
    let operation = if record.is_tombstone() {
        "delete"
    } else {
        "upsert"
    };
    for part in [kind, &id.to_string(), operation] {
        digest.update(part.as_bytes());
        digest.update(&[0]);
    }
    digest.update(original_body.as_bytes());
    let mutation_id = MutationId::new();
    let frozen = FrozenMutation {
        candidate: MutationCandidate {
            mutation_id,
            origin_device_id: author,
            origin_seq: sequence,
            previous_mutation_id: previous.map(|old| old.mutation_id),
            object: record.object_key()?,
            base_object_version: None,
            payload,
            resource_refs: vec![],
        },
        operation: if record.is_tombstone() {
            MutationOperation::Delete
        } else {
            MutationOperation::Upsert
        },
        local_revision: record.revision(),
        frozen_at_ms: now,
        original_body,
        original_resources_body: "[]".to_owned(),
        payload_digest: digest.finalize().to_hex().to_string(),
    };
    let body_digest = frozen.content_hash()?;
    let disposition = MutationDisposition {
        author,
        mutation_id,
        frozen_body_digest: body_digest.clone(),
        status: MutationStatus::Pending,
        outcome: None,
    };
    Ok(vec![
        DurableStateChange::scoped(
            StateKey {
                namespace: namespace.clone(),
                target: StateTarget::Mutation {
                    author,
                    mutation_id,
                    body_digest: body_digest.clone(),
                },
            },
            json!({"schema":"echo.sync.frozen-mutation.v2", "frozen":frozen}),
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
            json!({"schema":"echo.sync.mutation-disposition.v2", "disposition":disposition}),
        )?,
    ])
}

fn members(
    replay: &LibraryReplay,
    playlist: uuid::Uuid,
    records: &[crate::domain::library::PortableRecordEnvelope],
) -> serde_json::Value {
    let mut members = std::collections::BTreeMap::new();
    for envelope in replay
        .checkpoint
        .iter()
        .flat_map(|image| &image.records)
        .chain(
            replay
                .commits
                .iter()
                .flat_map(|commit| &commit.batch.records),
        )
        .chain(records)
    {
        match envelope.record() {
            PortableRecord::PlaylistItem(item) if item.playlist_uuid == playlist => {
                members.insert(item.item_uuid, item.clone());
            }
            PortableRecord::Tombstone(record)
                if record.deleted_kind == RecordKind::PlaylistItem =>
            {
                members.remove(&record.object_uuid);
            }
            _ => {}
        }
    }
    let mut ordered: Vec<_> = members.into_values().collect();
    ordered.sort_by_key(|item| (item.position, item.song_uuid));
    json!(ordered
        .into_iter()
        .map(|item| json!({
            "member_uuid":item.item_uuid, "song_uuid":item.song_uuid,
            "position":item.position, "added_at":item.added_at,
        }))
        .collect::<Vec<_>>())
}

fn encode(error: serde_json::Error) -> Error {
    Error::unavailable("test mutation", error.to_string())
}
