//! Complete state keys survive materialization and coverage compaction.
use super::*;
use crate::domain::library::durability::{StateKey, StateNamespace, StateTarget};
use crate::domain::sync::{AuthorityId, Epoch, LibraryId as SyncLibraryId, ObjectKey, SyncBinding};

#[test]
fn library_durability_scoped_state_same_object_across_binding_epochs_survives_compaction() {
    let (_dir, registry, root, library, _plane) = fixture();
    let object = ObjectKey::Song(crate::domain::ids::SongId::new());
    let ObjectKey::Song(id) = object else {
        unreachable!()
    };
    let binding = SyncBinding {
        library_id: SyncLibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let mut states = Vec::new();
    for index in 0..3 {
        let mut current = binding;
        if index == 1 {
            current.epoch = Epoch::from_u64(2);
        }
        if index == 2 {
            current.authority_id = AuthorityId::new();
        }
        states.push(DurableStateChange::scoped(StateKey { namespace:StateNamespace::Bound { binding:current }, target:StateTarget::Confirmed { object } }, serde_json::json!({"object_type":"song","object_uuid":id,"title":format!("context {index}")})).unwrap());
    }
    for index in 0..35 {
        let mut batch =
            LibraryBatch::new(uuid::Uuid::new_v4(), library, ChangeOrigin::Remote, vec![]);
        batch.state.push(states[index % 3].clone());
        commit(&registry, root, &batch).unwrap();
    }
    let replay = chain(&registry, root).unwrap();
    let image = replay.checkpoint.unwrap();
    assert_eq!(image.state.len(), 3);
    for state in states {
        assert!(image.state.contains(&state));
        let path = checkpoint::state_path(&state);
        let raw = portable::read(&registry, root, &path, LIMIT)
            .unwrap()
            .unwrap();
        assert_eq!(decode::<DurableStateChange>(&raw).unwrap(), state);
    }
}

#[test]
fn library_durability_scoped_reanchor_keeps_both_frozen_versions_and_late_disposition() {
    use crate::domain::library::{
        PortableSerialize,
        durability::{FrozenMutation, MutationDisposition, MutationOperation, MutationStatus},
    };
    use crate::domain::sync::{DeviceId, MutationCandidate, MutationId};
    let (_dir, registry, root, library, _plane) = fixture();
    let author = DeviceId::new();
    let mutation_id = MutationId::new();
    let namespace = StateNamespace::LegacyEvidence { source_root: root };
    let mut body = FrozenMutation {
        candidate: MutationCandidate {
            mutation_id,
            origin_device_id: author,
            origin_seq: 1,
            previous_mutation_id: None,
            object: ObjectKey::Song(crate::domain::ids::SongId::new()),
            base_object_version: None,
            payload: serde_json::json!({"title":"original"}),
            resource_refs: vec![],
        },
        operation: MutationOperation::Upsert,
        local_revision: Revision::from_u64(2),
        frozen_at_ms: 100,
        original_body: "{\"title\":\"original\"}".into(),
        original_resources_body: "[]".into(),
        payload_digest: "a".repeat(64),
    };
    let first_digest = body.content_hash().unwrap();
    let first = DurableStateChange::scoped(
        StateKey {
            namespace: namespace.clone(),
            target: StateTarget::Mutation {
                author,
                mutation_id,
                body_digest: first_digest.clone(),
            },
        },
        serde_json::json!({"frozen":body}),
    )
    .unwrap();
    body.candidate.origin_seq = 8;
    body.candidate.previous_mutation_id = Some(MutationId::new());
    let second_digest = body.content_hash().unwrap();
    let second = DurableStateChange::scoped(
        StateKey {
            namespace: namespace.clone(),
            target: StateTarget::Mutation {
                author,
                mutation_id,
                body_digest: second_digest,
            },
        },
        serde_json::json!({"frozen":body}),
    )
    .unwrap();
    let disposition = MutationDisposition {
        author,
        mutation_id,
        frozen_body_digest: first_digest.clone(),
        status: MutationStatus::Confirmed,
        outcome: Some(serde_json::json!({"sequence":9})),
    };
    let late = DurableStateChange::scoped(
        StateKey {
            namespace,
            target: StateTarget::MutationDisposition {
                author,
                mutation_id,
                body_digest: first_digest,
            },
        },
        serde_json::json!({"disposition":disposition}),
    )
    .unwrap();
    for index in 0..35 {
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            library,
            ChangeOrigin::LocalEdit,
            vec![],
        );
        if index == 0 {
            batch.state.push(first.clone());
        }
        if index == 1 {
            batch.state.push(second.clone());
        }
        if index == 2 {
            batch.state.push(late.clone());
        }
        commit(&registry, root, &batch).unwrap();
    }
    let replay = chain(&registry, root).unwrap();
    let image = replay.checkpoint.unwrap();
    assert_eq!(image.state.len(), 3);
    for state in [first, second, late] {
        assert!(image.state.contains(&state));
    }
}
