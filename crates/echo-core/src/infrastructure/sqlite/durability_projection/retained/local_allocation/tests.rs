//! Allocation recovery validates complete local facts without authorizing sends.
use super::*;
use crate::domain::ids::{PlaylistId, Revision};
use crate::domain::library::PortableSerialize;
use crate::domain::library::durability::{
    MutationDisposition, MutationOperation, MutationStatus, StateKey,
};
use crate::domain::sync::{DeviceId, MutationCandidate, MutationId, ObjectKey};
use crate::infrastructure::sqlite::SqliteDatabase;
use serde_json::json;

fn pair(library: LibraryId, author: DeviceId) -> Vec<DurableStateChange> {
    let payload = json!({"display_name":"allocation"});
    let frozen = FrozenMutation {
        candidate: MutationCandidate {
            mutation_id: MutationId::new(),
            origin_device_id: author,
            origin_seq: 1,
            previous_mutation_id: None,
            object: ObjectKey::Playlist(PlaylistId::new()),
            base_object_version: None,
            payload: payload.clone(),
            resource_refs: vec![],
        },
        operation: MutationOperation::Upsert,
        local_revision: Revision::from_u64(1),
        frozen_at_ms: 123,
        original_body: payload.to_string(),
        original_resources_body: "[]".into(),
        payload_digest: "a".repeat(64),
    };
    frozen_pair(library, &frozen)
}

fn frozen_pair(library: LibraryId, frozen: &FrozenMutation) -> Vec<DurableStateChange> {
    let author = frozen.candidate.origin_device_id;
    let digest = frozen.content_hash().unwrap();
    let mutation_id = frozen.candidate.mutation_id;
    let namespace = StateNamespace::LocalUnbound {
        library_id: library,
        author,
    };
    vec![
        DurableStateChange::scoped(StateKey {
            namespace: namespace.clone(),
            target: StateTarget::Mutation { author, mutation_id, body_digest: digest.clone() },
        }, json!({"schema":"echo.sync.frozen-mutation.v2","frozen":frozen})).unwrap(),
        DurableStateChange::scoped(StateKey {
            namespace,
            target: StateTarget::MutationDisposition { author, mutation_id, body_digest: digest.clone() },
        }, json!({"schema":"echo.sync.mutation-disposition.v2","disposition":MutationDisposition {
            author, mutation_id, frozen_body_digest: digest, status: MutationStatus::Pending, outcome: None,
        }})).unwrap(),
    ]
}

#[test]
fn library_durability_local_allocation_unproven_predecessor_never_advances_fences() {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let states = pair(library, author);
    let mut frozen: FrozenMutation =
        serde_json::from_value(states[0].payload["frozen"].clone()).unwrap();
    frozen.candidate.origin_seq = 2;
    frozen.candidate.previous_mutation_id = Some(MutationId::new());
    let digest = frozen.content_hash().unwrap();
    let states: Vec<_> = states
        .into_iter()
        .map(|state| {
            let mut key = state.key.unwrap();
            let mut payload = state.payload;
            match &mut key.target {
                StateTarget::Mutation { body_digest, .. } => {
                    body_digest.clone_from(&digest);
                    payload["frozen"] = serde_json::to_value(&frozen).unwrap();
                }
                StateTarget::MutationDisposition { body_digest, .. } => {
                    body_digest.clone_from(&digest);
                    payload["disposition"]["frozen_body_digest"] = json!(digest);
                }
                _ => unreachable!(),
            }
            DurableStateChange::scoped(key, payload).unwrap()
        })
        .collect();
    db.writer.run(move |c| {
        let root = LibraryRootId::new();
        for state in &states {
            assert_eq!(restore(c, root, Some(library), state, &states)?, Some(Reason::MissingSyncDependency));
        }
        let fences: u64 = c.query_row("SELECT (SELECT COUNT(*) FROM sync_local_highwater)+(SELECT COUNT(*) FROM sync_local_origin_cursors)", [], |r|r.get(0)).map_err(storage)?;
        assert_eq!(fences, 0);
        Ok(())
    }).unwrap();
}

#[test]
fn library_durability_local_allocation_resource_evidence_remains_unresolved() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let original = pair(library, author);
    let mut frozen: FrozenMutation =
        serde_json::from_value(original[0].payload["frozen"].clone()).unwrap();
    frozen.candidate.resource_refs.push(
        SharedResourceManifest::new(
            SharedResourceKind::PlaylistCover,
            ResourceDigest::from_bytes([3; 32]),
            12,
            "image/png".into(),
            "covers/cover.png",
        )
        .unwrap(),
    );
    frozen.original_resources_body =
        serde_json::to_string(&frozen.candidate.resource_refs).unwrap();
    let mut states = frozen_pair(library, &frozen);
    // Resource recovery does not use other allocations as proof. A corrupt
    // unrelated allocation must therefore not bypass this early disposition.
    let mut unrelated = pair(library, author).remove(0);
    unrelated.payload["frozen"]["payload_digest"] = json!("invalid");
    states.push(unrelated);
    let (reasons, fences) = db.writer.run(move |c| {
        let root = LibraryRootId::new();
        let mut index = AllocationIndex::new(&states)?;
        let reasons = states[..2].iter().map(|state| restore_indexed(c, root, Some(library), state, &mut index)).collect::<Result<Vec<_>, _>>()?;
        let fences: u64 = c.query_row("SELECT (SELECT COUNT(*) FROM sync_local_highwater)+(SELECT COUNT(*) FROM sync_local_origin_cursors)", [], |r|r.get(0)).map_err(storage)?;
        Ok((reasons, fences))
    }).unwrap();
    assert_eq!(reasons, vec![Some(Reason::RequiresStateRecovery); 2]);
    assert_eq!(fences, 0);
}

#[test]
fn library_durability_local_allocation_missing_pair_never_advances_fences() {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let states = pair(library, author);
    db.writer.run(move |c| {
        for state in &states {
            assert_eq!(restore(c, LibraryRootId::new(), Some(library), state, std::slice::from_ref(state))?, Some(Reason::MissingSyncDependency));
        }
        let fences: u64 = c.query_row("SELECT (SELECT COUNT(*) FROM sync_local_highwater)+(SELECT COUNT(*) FROM sync_local_origin_cursors)", [], |r|r.get(0)).map_err(storage)?;
        assert_eq!(fences, 0);
        Ok(())
    }).unwrap();
}

#[test]
fn library_durability_local_allocation_ambiguous_frozen_body_never_advances_fences() {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let mut states = pair(library, author);
    let mut changed: FrozenMutation =
        serde_json::from_value(states[0].payload["frozen"].clone()).unwrap();
    changed.candidate.payload["display_name"] = json!("different frozen body");
    changed.original_body = changed.candidate.payload.to_string();
    states.extend(frozen_pair(library, &changed));
    let (reasons, fences) = db.writer.run(move |c| {
        let root = LibraryRootId::new();
        let reasons = states.iter().map(|state| restore(c, root, Some(library), state, &states)).collect::<Result<Vec<_>, _>>()?;
        let fences: u64 = c.query_row("SELECT (SELECT COUNT(*) FROM sync_local_highwater)+(SELECT COUNT(*) FROM sync_local_origin_cursors)", [], |r|r.get(0)).map_err(storage)?;
        Ok((reasons, fences))
    }).unwrap();
    assert_eq!(reasons, vec![Some(Reason::IdentityConflict); 4]);
    assert_eq!(fences, 0);
}

#[test]
fn library_durability_local_allocation_cross_namespace_sequence_conflict_is_retained() {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let mut states = pair(library, author);
    let other = pair(library, author).into_iter().map(|state| {
        let mut key = state.key.unwrap();
        key.namespace = StateNamespace::LegacyEvidence {
            source_root: LibraryRootId::new(),
        };
        DurableStateChange::scoped(key, state.payload).unwrap()
    });
    states.extend(other);
    db.writer.run(move |c| {
        for state in &states[..2] {
            assert_eq!(restore(c, LibraryRootId::new(), Some(library), state, &states)?, Some(Reason::IdentityConflict));
        }
        let fences: u64 = c.query_row("SELECT (SELECT COUNT(*) FROM sync_local_highwater)+(SELECT COUNT(*) FROM sync_local_origin_cursors)", [], |r|r.get(0)).map_err(storage)?;
        assert_eq!(fences, 0);
        Ok(())
    }).unwrap();
}

#[test]
fn library_durability_local_allocation_reuses_one_index_for_long_history() {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("allocation.sqlite")).unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let author = DeviceId::from_uuid(db.device_id.as_uuid());
    let original = pair(library, author);
    let template: FrozenMutation =
        serde_json::from_value(original[0].payload["frozen"].clone()).unwrap();
    let mut states = Vec::new();
    let mut previous = None;
    for sequence in 1..=128 {
        let mut frozen = template.clone();
        frozen.candidate.mutation_id = MutationId::new();
        frozen.candidate.origin_seq = sequence;
        frozen.candidate.previous_mutation_id = previous;
        frozen.local_revision = Revision::from_u64(sequence);
        previous = Some(frozen.candidate.mutation_id);
        states.extend(frozen_pair(library, &frozen));
    }
    db.writer
        .run(move |c| {
            let root = LibraryRootId::new();
            let mut index = AllocationIndex::new(&states)?;
            for state in &states {
                assert_eq!(restore_indexed(c, root, Some(library), state, &mut index)?, None);
            }
            // Frozen evidence is validated once for the whole history, and both
            // halves of every pair share the same author conflict/predecessor proof.
            assert_eq!(index.proof_counts(), (states.len(), 1));
            let cursor: (u64, String) = c.query_row(
                "SELECT origin_seq,last_mutation_id FROM sync_local_origin_cursors WHERE local_root_uuid=?1 AND origin_device_id=?2",
                params![root.to_string(), author.as_uuid().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(storage)?;
            assert_eq!(cursor, (128, previous.unwrap().as_uuid().to_string()));
            Ok(())
        })
        .unwrap();
}
