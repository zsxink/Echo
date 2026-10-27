//! Snapshot roots remain bounded while immutable history and resource holds survive.
use super::*;
use crate::domain::library::PortableSerialize;

#[test]
fn library_durability_snapshot_reuses_durable_unchanged_shards() {
    let (_directory, registry, root, library, _) = fixture();
    let mut seed = batch(library, uuid::Uuid::new_v4(), "seed shard");
    for _ in 0..127 {
        seed.records
            .extend(batch(library, uuid::Uuid::new_v4(), "another shard").records);
    }
    commit(&registry, root, &seed).unwrap();
    let mut trace = Vec::new();
    commit_with_hook(
        &registry,
        root,
        &batch(library, uuid::Uuid::new_v4(), "one changed shard"),
        &mut |phase, _| {
            trace.push(phase.to_owned());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        trace.iter().filter(|p| *p == "snapshot:file_sync").count(),
        1
    );
    assert_eq!(
        trace
            .iter()
            .filter(|p| *p == "snapshot_retry:file_sync")
            .count(),
        0,
        "unchanged leaves already referenced by the durable checkpoint need no new barriers"
    );
    assert_eq!(recover(&registry, root).unwrap().head().unwrap().0, 2);
}

#[test]
fn library_durability_snapshot_retry_reflushes_unreferenced_visible_leaf() {
    let (_directory, registry, root, _, _) = fixture();
    let mut values = BTreeMap::from([("key".to_owned(), "original".to_owned())]);
    let durable = snapshot::publish(&registry, root, &values, &mut |_, _| Ok(())).unwrap();
    values.insert("key".to_owned(), "changed".to_owned());
    snapshot::publish_reusing(&registry, root, &values, Some(&durable), &mut |phase, _| {
        if phase == "snapshot:rename" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let mut trace = Vec::new();
    snapshot::publish_reusing(&registry, root, &values, Some(&durable), &mut |phase, _| {
        trace.push(phase.to_owned());
        Ok(())
    })
    .unwrap();
    assert!(trace.iter().any(|p| p == "snapshot_retry:file_sync"));
    assert!(trace.iter().any(|p| p == "snapshot_retry:directory_sync"));
}

#[test]
fn library_durability_snapshot_reused_proof_still_checks_leaf_bytes() {
    let (directory, registry, root, _, _) = fixture();
    let values = BTreeMap::from([("key".to_owned(), "original".to_owned())]);
    let durable = snapshot::publish(&registry, root, &values, &mut |_, _| Ok(())).unwrap();
    let raw = serde_json::to_value(&durable).unwrap();
    let digest = raw["shards"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .as_str()
        .unwrap();
    let path = directory
        .path()
        .join(format!("echo/state/checkpoint-chunks/{digest}.json"));
    std::fs::write(path, "tampered leaf").unwrap();
    assert!(
        snapshot::publish_reusing(&registry, root, &values, Some(&durable), &mut |_, _| Ok(()),)
            .is_err()
    );
}

#[test]
fn library_durability_snapshot_crash_temporary_does_not_block_repeated_compaction() {
    let (directory, registry, root, library, _) = fixture();
    commit(
        &registry,
        root,
        &batch(library, uuid::Uuid::new_v4(), "saved"),
    )
    .unwrap();
    let bytes = br#"{"prepared":"unique interrupted snapshot bytes"}"#;
    let name = format!(
        "{}.tmp-{}",
        blake3::hash(bytes).to_hex(),
        uuid::Uuid::new_v4()
    );
    let path = directory
        .path()
        .join("echo/state/checkpoint-chunks")
        .join(name);
    std::fs::write(&path, bytes).unwrap();
    for _ in 0..2 {
        let replay = recover(&registry, root).unwrap();
        checkpoint::compact(
            &registry,
            &ValidationCache::default(),
            root,
            &replay,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(chain(&registry, root).unwrap().head().unwrap().0, 1);
    }
}

#[test]
fn library_durability_snapshot_unknown_and_corrupt_unreferenced_evidence_is_preserved() {
    for name in [
        "unrecognized.bin".to_owned(),
        format!("{}.json", "a".repeat(64)),
    ] {
        let (directory, registry, root, library, _) = fixture();
        let change = batch(library, uuid::Uuid::new_v4(), "saved");
        commit(&registry, root, &change).unwrap();
        let path = directory
            .path()
            .join("echo/state/checkpoint-chunks")
            .join(name);
        std::fs::write(&path, "unknown original").unwrap();
        assert!(
            checkpoint::compact(
                &registry,
                &ValidationCache::default(),
                root,
                &chain(&registry, root).unwrap(),
                &mut |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "unknown original");
        assert!(marker_present(directory.path(), change.operation_id));
    }
}

#[test]
fn library_durability_snapshot_reads_v1_roots_without_rewriting_and_upgrades_on_commit() {
    let (directory, registry, root, library, _) = fixture();
    commit(
        &registry,
        root,
        &batch(library, uuid::Uuid::new_v4(), "legacy root"),
    )
    .unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &chain(&registry, root).unwrap(),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let image = chain(&registry, root).unwrap().checkpoint.unwrap();
    let materialized = super::super::materialization_checkpoint(&registry, root).unwrap();
    let roots = [
        (ANCHOR, serde_json::to_vec(&serde_json::json!({"version":1,"digest":image.content_hash().unwrap(),"image":image})).unwrap()),
        (CHECKPOINT, serde_json::to_vec(&serde_json::json!({"version":1,"seq":materialized.seq,"head":materialized.head,"files":materialized.files})).unwrap()),
    ];
    for (path, bytes) in &roots {
        std::fs::write(directory.path().join(path), bytes).unwrap();
    }
    assert_eq!(chain(&registry, root).unwrap().head().unwrap().0, 1);
    for (path, bytes) in &roots {
        assert_eq!(std::fs::read(directory.path().join(path)).unwrap(), *bytes);
    }
    commit(
        &registry,
        root,
        &batch(library, uuid::Uuid::new_v4(), "new root"),
    )
    .unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join(CHECKPOINT)).unwrap()).unwrap();
    assert_eq!(current["version"], 2);
    assert_eq!(recover(&registry, root).unwrap().head().unwrap().0, 2);
}

#[test]
fn library_durability_snapshot_roots_bound_history_and_collect_only_unreachable_leaves() {
    let (directory, registry, root, library, plane) = fixture();
    let object = uuid::Uuid::new_v4();
    let original = b"unique candidate, conflict, deletion and publication original";
    let asset = LibraryDurabilityPort::write_asset(&plane, root, original).unwrap();
    let mut expected = Vec::new();
    for index in 0..64_u128 {
        let mut change = batch(library, object, "same live object");
        if index == 0 {
            for kind in [
                StateKind::Pending,
                StateKind::Conflict,
                StateKind::DeleteIntent,
                StateKind::ResourceLocation,
                StateKind::SyncLedger,
            ] {
                let state = DurableStateChange::new(
                    kind,
                    uuid::Uuid::new_v4(),
                    serde_json::json!({"original_resource":asset,"snapshot":"retained"}),
                );
                expected.push(state.clone());
                change.state.push(state);
            }
            change.resources.push(DurableResource {
                relative_path: format!("echo/assets/{asset}"),
                content_hash: asset.clone(),
                byte_length: original.len() as u64,
            });
        }
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
    assert_eq!(
        std::fs::read(directory.path().join(format!("echo/assets/{asset}"))).unwrap(),
        original
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
    assert!(
        !directory
            .path()
            .join(relative(excessive.operation_id, "prepared.json"))
            .exists()
    );
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
