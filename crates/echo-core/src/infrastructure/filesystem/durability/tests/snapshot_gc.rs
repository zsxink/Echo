//! Snapshot roots remain bounded while immutable history and resource holds survive.
use super::*;

#[test]
fn library_durability_snapshot_roots_bound_history_and_collect_only_unreachable_leaves() {
    let (directory, registry, root, library, _) = fixture();
    let object = uuid::Uuid::new_v4();
    let mut expected = Vec::new();
    for index in 0..64_u128 {
        let mut change = batch(library, object, "same live object");
        let state = DurableStateChange::new(
            StateKind::PlaybackEvent,
            uuid::Uuid::from_u128(index + 1),
            serde_json::json!({"song_uuid": object, "recorded_at": index, "retained": "x".repeat(1024)}),
        );
        expected.push(state.clone());
        change.state.push(state);
        commit(&registry, root, &change).unwrap();
    }
    let replay = recover(&registry, root).unwrap();
    assert!(replay.commits.len() < WINDOW);
    let states: Vec<_> = replay.states().cloned().collect();
    for state in expected {
        assert!(states.contains(&state), "idempotence evidence survives");
    }
    let mut protected = std::collections::BTreeSet::new();
    for path in [ANCHOR, CHECKPOINT] {
        let raw = std::fs::read(directory.path().join(path)).unwrap();
        assert!(
            raw.len() < 32 * 1024,
            "root stores bounded shard references"
        );
        let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value["version"], 2);
        for field in [
            "record_snapshot",
            "state_snapshot",
            "resource_snapshot",
            "files_snapshot",
        ] {
            if let Some(shards) = value[field]["shards"].as_object() {
                protected.extend(
                    shards
                        .values()
                        .map(|hash| format!("{}.json", hash.as_str().unwrap())),
                );
            }
        }
    }
    let leaves: std::collections::BTreeSet<_> =
        std::fs::read_dir(directory.path().join("echo/state/checkpoint-chunks"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
    assert_eq!(
        leaves, protected,
        "only the current root graph remains after compaction"
    );
}

#[test]
fn library_durability_snapshot_capacity_refuses_before_marker_and_preserves_head() {
    let (directory, registry, root, library, _) = fixture();
    let initial = batch(library, uuid::Uuid::new_v4(), "saved");
    commit(&registry, root, &initial).unwrap();
    let mut excessive = batch(library, uuid::Uuid::new_v4(), "too large");
    excessive.state.push(DurableStateChange::new(
        StateKind::Conflict,
        uuid::Uuid::new_v4(),
        serde_json::json!({"original": "x".repeat(4 * 1024 * 1024)}),
    ));
    let error = commit(&registry, root, &excessive).unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert!(!marker_present(directory.path(), excessive.operation_id));
    assert!(!directory
        .path()
        .join(relative(excessive.operation_id, "prepared.json"))
        .exists());
    assert_eq!(recover(&registry, root).unwrap().head().unwrap().0, 1);
}

#[test]
fn library_durability_snapshot_missing_or_tampered_leaf_is_preserved_and_blocks_writes() {
    for missing in [false, true] {
        let (directory, registry, root, library, _) = fixture();
        let change = batch(library, uuid::Uuid::new_v4(), "covered");
        commit(&registry, root, &change).unwrap();
        let leaf = std::fs::read_dir(directory.path().join("echo/state/checkpoint-chunks"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        if missing {
            std::fs::remove_file(&leaf).unwrap();
        } else {
            std::fs::write(&leaf, "{}").unwrap();
        }
        let next = batch(library, uuid::Uuid::new_v4(), "blocked");
        assert!(commit(&registry, root, &next).is_err());
        assert!(!marker_present(directory.path(), next.operation_id));
        if !missing {
            assert_eq!(std::fs::read_to_string(&leaf).unwrap(), "{}");
        }
    }
}
