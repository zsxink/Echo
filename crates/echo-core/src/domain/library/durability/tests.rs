//! Contract regressions for scoped history and exact legacy journal bytes.
use super::*;
use crate::domain::{
    ids::{LibraryRootId, Revision, SongId},
    sync::{
        AuthorityId, DeviceId, Epoch, LibraryId as SyncLibraryId, MutationCandidate, MutationId,
        ObjectKey, SyncBinding,
    },
};
use serde_json::json;

fn namespace(epoch: u64) -> StateNamespace {
    StateNamespace::Bound {
        binding: SyncBinding {
            library_id: SyncLibraryId::from_uuid(uuid::Uuid::from_u128(1)),
            authority_id: AuthorityId::from_uuid(uuid::Uuid::from_u128(2)),
            epoch: Epoch::from_u64(epoch),
        },
    }
}
fn frozen(namespace: StateNamespace) -> DurableStateChange {
    let author = DeviceId::from_uuid(uuid::Uuid::from_u128(3));
    let mutation_id = MutationId::from_uuid(uuid::Uuid::from_u128(4));
    let body = FrozenMutation {
        candidate: MutationCandidate {
            mutation_id,
            origin_device_id: author,
            origin_seq: 8,
            previous_mutation_id: Some(MutationId::from_uuid(uuid::Uuid::from_u128(9))),
            object: ObjectKey::Song(SongId::from_uuid(uuid::Uuid::from_u128(5))),
            base_object_version: None,
            payload: json!({"title":"value"}),
            resource_refs: vec![],
        },
        operation: MutationOperation::Upsert,
        local_revision: Revision::from_u64(7),
        frozen_at_ms: 1234,
        original_body: "{ \"title\" : \"value\" }".into(),
        original_resources_body: "[]".into(),
        payload_digest: "a".repeat(64),
    };
    let key = StateKey {
        namespace,
        target: StateTarget::Mutation {
            author,
            mutation_id,
            body_digest: body.content_hash().unwrap(),
        },
    };
    DurableStateChange::scoped(
        key,
        json!({"schema":"echo.sync.frozen-mutation.v2","frozen":body}),
    )
    .unwrap()
}
#[test]
fn library_durability_scoped_key_retains_same_event_across_bindings_and_epochs() {
    let first = frozen(namespace(1));
    let second = frozen(namespace(2));
    let mut other = namespace(1);
    if let StateNamespace::Bound { binding } = &mut other {
        binding.authority_id = AuthorityId::new();
    }
    let third = frozen(other);
    assert_ne!(first.id, second.id);
    assert_ne!(first.id, third.id);
    assert_ne!(first.storage_key().unwrap(), second.storage_key().unwrap());
    assert_eq!(first.payload["frozen"], second.payload["frozen"]);
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        LibraryId::from_uuid(uuid::Uuid::new_v4()),
        ChangeOrigin::LocalEdit,
        vec![],
    );
    batch.state = vec![first, second, third];
    batch.validate().unwrap();
}
#[test]
fn library_durability_scoped_key_rejects_changed_id_key_payload_or_frozen_identity() {
    let original = frozen(namespace(1));
    for field in ["id", "key", "payload_key", "body", "author"] {
        let mut state = original.clone();
        match field {
            "id" => state.id = uuid::Uuid::new_v4(),
            "key" => state.key.as_mut().unwrap().namespace = namespace(2),
            "payload_key" => state.payload["state_key"]["namespace"] = json!(namespace(2)),
            "body" => state.payload["frozen"]["original_body"] = json!("{}"),
            "author" => {
                state.payload["frozen"]["candidate"]["origin_device_id"] = json!(DeviceId::new());
            }
            _ => unreachable!(),
        }
        assert!(state.validate().is_err(), "{field}");
    }
    let key = StateKey {
        namespace: namespace(1),
        target: StateTarget::Confirmed {
            object: ObjectKey::Song(SongId::new()),
        },
    };
    assert!(DurableStateChange::scoped(
        key,
        json!({"object_type":"song","object_uuid":uuid::Uuid::new_v4()})
    )
    .is_err());
}
#[test]
fn library_durability_frozen_body_is_separate_from_mutable_disposition() {
    let first = frozen(namespace(1));
    let key = first.key.as_ref().unwrap();
    let StateTarget::Mutation {
        author,
        mutation_id,
        ref body_digest,
    } = key.target
    else {
        panic!()
    };
    let disposition_key = StateKey {
        namespace: key.namespace.clone(),
        target: StateTarget::MutationDisposition {
            author,
            mutation_id,
            body_digest: body_digest.clone(),
        },
    };
    let mut disposition = MutationDisposition {
        author,
        mutation_id,
        frozen_body_digest: body_digest.clone(),
        status: MutationStatus::Pending,
        outcome: None,
    };
    let pending =
        DurableStateChange::scoped(disposition_key.clone(), json!({"disposition":disposition}))
            .unwrap();
    disposition.status = MutationStatus::Confirmed;
    disposition.outcome = Some(json!({"sequence":88}));
    let confirmed =
        DurableStateChange::scoped(disposition_key, json!({"disposition":disposition})).unwrap();
    assert_eq!(pending.id, confirmed.id);
    assert_ne!(first.id, pending.id);
    let image = LibraryCheckpoint {
        library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
        library_commit_seq: 1,
        head_digest: "a".repeat(64),
        records: vec![],
        state: vec![first.clone(), pending],
        resources: vec![],
    };
    let replay = LibraryReplay {
        checkpoint: Some(image),
        commits: vec![],
    };
    replay
        .validate_state_extension(&[first.clone(), confirmed])
        .unwrap();
    let mut rewritten = first.clone();
    rewritten.payload["frozen"]["frozen_at_ms"] = json!(9999);
    assert!(rewritten.validate().is_err());
    assert!(replay.validate_state_extension(&[rewritten]).is_err());
    assert_eq!(
        first.payload["frozen"]["original_body"],
        "{ \"title\" : \"value\" }"
    );
}
#[test]
fn library_durability_legacy_committed_canonical_digest_is_unchanged() {
    // This is exactly the old serializer's shape; the new key must stay absent.
    const LEGACY_BATCH: &str = r#"{"operation_id":"00000000-0000-0000-0000-000000000011","library_id":"00000000-0000-0000-0000-000000000012","origin":"recovery","records":[],"state":[{"kind":"pending","id":"00000000-0000-0000-0000-000000000013","payload":{"original":"bytes","status":"sent"}}],"resources":[],"expected":[]}"#;
    let legacy_commit_payload = format!("[{LEGACY_BATCH},3,\"{}\"]", "a".repeat(64));
    let digest = blake3::hash(legacy_commit_payload.as_bytes())
        .to_hex()
        .to_string();
    assert_eq!(
        digest,
        "68910618ffce0826286842645951086c2810523073341a951e00536b0b8be84c"
    );
    let batch: LibraryBatch = serde_json::from_str(LEGACY_BATCH).unwrap();
    assert_eq!(batch.to_canonical_json().unwrap(), LEGACY_BATCH);
    assert!(batch.state[0].key.is_none());
    let commit = LibraryCommit {
        batch,
        library_commit_seq: 3,
        predecessor_digest: Some("a".repeat(64)),
        digest,
    };
    commit.validate().unwrap();
}

#[test]
fn library_durability_protocol_keeps_unknown_historical_times_and_confirmation_absent() {
    use crate::domain::library::{PortableRecord, PortableRecordDocument};

    let common = json!({
        "revision": 1,
        "updated_by_device_id": "00000000-0000-0000-0000-000000000002",
        "hlc": {"wall_secs": 1_700_000_000_u64, "counter": 0}
    });
    let mut records = Vec::new();
    for (kind, fields, absent) in [
        (
            "playlist",
            json!({"playlist_uuid": "00000000-0000-0000-0000-000000000003", "display_name": "Historical"}),
            vec!["created_at", "cover_asset"],
        ),
        (
            "playlist_item",
            json!({"item_uuid": "00000000-0000-0000-0000-000000000004", "playlist_uuid": "00000000-0000-0000-0000-000000000003", "song_uuid": "00000000-0000-0000-0000-000000000005", "position": 7}),
            vec!["added_at"],
        ),
    ] {
        let mut raw = common.clone();
        raw["type"] = json!(kind);
        raw.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let document: PortableRecordDocument = serde_json::from_value(raw).unwrap();
        assert!(matches!(document, PortableRecordDocument::V1(_)));
        let envelope = PortableRecordEnvelope::new(document.into_record(), None).unwrap();
        let encoded = serde_json::to_value(&envelope).unwrap();
        for field in absent {
            assert!(encoded["record"].get(field).is_none(), "fabricated {field}");
        }
        assert!(encoded.get("sync").is_none());
        records.push(envelope);
    }
    assert!(matches!(records[0].record(), PortableRecord::Playlist(_)));
    let batch = LibraryBatch::new(
        uuid::Uuid::from_u128(10),
        LibraryId::from_uuid(uuid::Uuid::from_u128(11)),
        ChangeOrigin::Recovery,
        records,
    );
    let commit = LibraryCommit {
        digest: LibraryCommit::calculate_digest(&batch, 1, None).unwrap(),
        batch,
        library_commit_seq: 1,
        predecessor_digest: None,
    };
    let round_trip: LibraryCommit =
        serde_json::from_str(&commit.to_canonical_json().unwrap()).unwrap();
    round_trip.validate().unwrap();
    assert_eq!(round_trip, commit);
}
#[test]
fn library_durability_legacy_scope_is_explicit_and_local_unbound_never_invents_binding() {
    let first = frozen(StateNamespace::LegacyEvidence {
        source_root: LibraryRootId::new(),
    });
    let second = frozen(StateNamespace::LegacyEvidence {
        source_root: LibraryRootId::new(),
    });
    assert_ne!(first.id, second.id);
    let local = frozen(StateNamespace::LocalUnbound {
        library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
        author: DeviceId::from_uuid(uuid::Uuid::from_u128(3)),
    });
    local.validate().unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        LibraryId::from_uuid(uuid::Uuid::new_v4()),
        ChangeOrigin::LocalEdit,
        vec![],
    );
    batch.state.push(local.clone());
    assert!(batch.validate().is_err());
    let StateNamespace::LocalUnbound { library_id, .. } = local.key.as_ref().unwrap().namespace
    else {
        panic!()
    };
    batch.library_id = library_id;
    batch.validate().unwrap();
    let mut invalid = local.key.unwrap();
    if let StateNamespace::LocalUnbound { author, .. } = &mut invalid.namespace {
        *author = DeviceId::new();
    }
    assert!(invalid.validate().is_err());
}

#[test]
fn library_durability_frozen_resource_body_must_match_all_typed_manifests() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let state = frozen(namespace(1));
    let mut body: FrozenMutation = serde_json::from_value(state.payload["frozen"].clone()).unwrap();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([4; 32]),
        7,
        "audio/flac".to_owned(),
        "media/original.flac",
    )
    .unwrap();
    body.candidate.resource_refs = vec![resource];
    let key_for = |body: &FrozenMutation| StateKey {
        namespace: namespace(1),
        target: StateTarget::Mutation {
            author: body.candidate.origin_device_id,
            mutation_id: body.candidate.mutation_id,
            body_digest: body.content_hash().unwrap(),
        },
    };
    // Recompute the version key so rejection proves original/typed disagreement,
    // independently of the already-covered stale version digest guard.
    assert!(body.validate(&key_for(&body)).is_err());
    body.original_resources_body =
        serde_json::to_string_pretty(&body.candidate.resource_refs).unwrap();
    body.validate(&key_for(&body)).unwrap();
    for field in ["kind", "digest", "length", "media_type", "relative_path"] {
        let mut changed = body.clone();
        let resource = &mut changed.candidate.resource_refs[0];
        match field {
            "kind" => resource.kind = SharedResourceKind::LyricsSidecar,
            "digest" => resource.digest = ResourceDigest::from_bytes([5; 32]),
            "length" => resource.length = 8,
            "media_type" => resource.media_type = "text/plain".to_owned(),
            "relative_path" => {
                resource.relative_path =
                    crate::domain::library::LibraryRelativePath::new("media/other.flac").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(changed.validate(&key_for(&changed)).is_err(), "{field}");
    }
    let mut erased = body.clone();
    erased.candidate.resource_refs.clear();
    assert!(erased.validate(&key_for(&erased)).is_err());
    let mut malformed = body;
    malformed.original_resources_body = "[{}]".to_owned();
    assert!(malformed.validate(&key_for(&malformed)).is_err());
}
