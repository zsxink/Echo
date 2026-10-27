//! Compression fault matrix, native locks and malformed directory evidence.
use super::*;

fn seed() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    LibraryId,
    LibraryReplay,
    String,
) {
    let (dir, registry, root, library, plane) = fixture();
    let hash = LibraryDurabilityPort::write_asset(&plane, root, b"unique bytes retained").unwrap();
    for i in 0..3 {
        let mut batch = batch(library, uuid::Uuid::from_u128(100), &format!("value {i}"));
        batch.state.push(DurableStateChange::new(
            StateKind::Pending,
            uuid::Uuid::from_u128(i + 1),
            serde_json::json!({"resource": hash, "original": i}),
        ));
        batch.resources.push(DurableResource {
            relative_path: format!("echo/assets/{hash}"),
            content_hash: hash.clone(),
            byte_length: 21,
        });
        commit(&registry, root, &batch).unwrap();
    }
    let replay = chain(&registry, root).unwrap();
    (dir, registry, root, library, replay, hash)
}

#[test]
fn library_durability_every_compaction_boundary_keeps_snapshot_and_unique_resources() {
    let (_dir, registry, root, _, replay, _) = seed();
    let mut trace = Vec::new();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |phase, index| {
            trace.push((phase.to_owned(), index));
            Ok(())
        },
    )
    .unwrap();
    assert!(
        trace
            .iter()
            .any(|(phase, _)| phase == "coverage:directory_sync")
    );
    assert!(trace.iter().any(|(phase, _)| phase == "gc:remove"));
    for required in [
        "snapshot:file_sync",
        "snapshot:rename",
        "snapshot:directory_sync",
        "snapshot_root_gc:file_sync",
        "snapshot_leaf_gc:file_sync",
        "snapshot_proof_gc:directory_sync",
        "snapshot_gc:remove",
        "snapshot_gc:directory_sync",
    ] {
        assert!(
            trace.iter().any(|(phase, _)| phase == required),
            "missing snapshot interruption boundary {required}"
        );
    }
    for (stop, boundary) in trace.iter().enumerate() {
        let (dir, registry, root, _, replay, hash) = seed();
        let mut counter = 0;
        assert!(
            checkpoint::compact(
                &registry,
                &ValidationCache::default(),
                root,
                &replay,
                &mut |_, _| {
                    let fail_here = counter == stop;
                    counter += 1;
                    if fail_here { Err(fail()) } else { Ok(()) }
                }
            )
            .is_err()
        );
        let restored = recover(&registry, root)
            .unwrap_or_else(|e| panic!("boundary {stop} {boundary:?}: {e}"));
        assert_eq!(restored.head().unwrap().0, 3);
        let states: std::collections::BTreeSet<_> = restored.states().map(|s| s.id).collect();
        assert_eq!(states.len(), 3, "pending original bodies survive GC {stop}");
        assert_eq!(
            std::fs::read(dir.path().join(format!("echo/assets/{hash}"))).unwrap(),
            b"unique bytes retained"
        );
        for original in &replay.commits {
            assert_eq!(
                read_operation(&registry, root, original.batch.operation_id)
                    .unwrap()
                    .unwrap(),
                *original
            );
        }
    }
}

#[test]
fn library_durability_corrupt_coverage_anchor_is_preserved_and_blocks_writes() {
    let (dir, registry, root, library, replay, _) = seed();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let path = dir.path().join(ANCHOR);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["image"]["library_commit_seq"] = serde_json::json!(999);
    let corrupt = serde_json::to_vec(&value).unwrap();
    std::fs::write(&path, &corrupt).unwrap();
    assert!(recover(&registry, root).is_err());
    assert!(
        commit(
            &registry,
            root,
            &batch(library, uuid::Uuid::new_v4(), "blocked")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), corrupt);
}

#[cfg(unix)]
#[test]
fn library_durability_symlink_unknown_format_and_invalid_resource_fail_before_marker() {
    let (dir, registry, root, library, plane) = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("echo/transactions")).unwrap();
    let change = batch(library, uuid::Uuid::new_v4(), "blocked");
    assert!(LibraryDurabilityPort::commit(&plane, root, &change).is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    std::fs::remove_file(dir.path().join("echo/transactions")).unwrap();
    let mut unknown = LibraryManifest::new(library, "future");
    unknown.format_version = u64::MAX;
    std::fs::write(
        dir.path().join("echo/manifest.json"),
        serde_json::to_vec(&unknown).unwrap(),
    )
    .unwrap();
    assert!(LibraryDurabilityPort::commit(&plane, root, &change).is_err());
    assert!(!marker_present(dir.path(), change.operation_id));
    assert!(
        registry
            .resolve_relative(
                root,
                &crate::domain::ids::RelativeMediaPath::new("echo/assets/x").unwrap()
            )
            .is_ok()
    );
}

#[test]
fn library_durability_duplicate_sequence_refuses_recovery_and_preserves_both() {
    let (dir, registry, root, library, _) = fixture();
    let first = batch(library, uuid::Uuid::new_v4(), "one");
    commit(&registry, root, &first).unwrap();
    let second = batch(library, uuid::Uuid::new_v4(), "two");
    let digest = LibraryCommit::calculate_digest(&second, 1, None).unwrap();
    let duplicate = LibraryCommit {
        batch: second.clone(),
        library_commit_seq: 1,
        predecessor_digest: None,
        digest: digest.clone(),
    };
    let folder = dir
        .path()
        .join(format!("echo/transactions/{}", second.operation_id));
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("prepared.json"),
        serde_json::to_vec(&duplicate).unwrap(),
    )
    .unwrap();
    std::fs::write(
        folder.join("committed.json"),
        serde_json::to_vec(&Marker {
            version: 1,
            seq: 1,
            digest,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(recover(&registry, root).is_err());
    assert!(marker_present(dir.path(), first.operation_id));
    assert!(marker_present(dir.path(), second.operation_id));
}

#[test]
fn library_durability_prepared_only_is_not_accepted_as_saved_recovery() {
    let (dir, registry, root, library, plane) = fixture();
    let pending = batch(library, uuid::Uuid::new_v4(), "uncommitted");
    commit_with_hook(&registry, root, &pending, &mut |phase, _| {
        if phase == "prepared" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(
        operation_status(&registry, root, pending.operation_id)
            .unwrap()
            .unwrap()
            .status,
        CommitStatus::NotSaved
    );
    let database_dir = tempfile::tempdir().unwrap();
    let database = crate::infrastructure::sqlite::SqliteDatabase::open(
        database_dir.path().join("projection.db"),
    )
    .unwrap();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, dir.path().to_owned(), true, true),
    )
    .unwrap();
    let error = crate::application::library_recovery::RetryLibraryOperation::new(&plane, &database)
        .execute(
            root,
            crate::domain::ids::OperationId::from_uuid(pending.operation_id),
        )
        .unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert!(!marker_present(dir.path(), pending.operation_id));
}

#[test]
fn library_durability_cross_process_writer_exclusion_and_reader_access() {
    let (dir, _registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "cross process");
    LibraryDurabilityPort::commit(&plane, root, &change).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("echo/write.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&file).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "infrastructure::filesystem::durability::tests::faults::library_durability_cross_process_child", "--nocapture"])
        .env("ECHO_DURABILITY_LOCK_TEST_ROOT", dir.path()).status().unwrap();
    assert!(status.success());
}

#[test]
fn library_durability_cross_process_child() {
    let Some(directory) = std::env::var_os("ECHO_DURABILITY_LOCK_TEST_ROOT") else {
        return;
    };
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, &directory);
    let plane = RootControlPlane::new(registry);
    assert_eq!(
        plane
            .list_records(root, RecordKind::Playlist)
            .unwrap()
            .len(),
        1
    );
    let library = plane.read_manifest(root).unwrap().unwrap().library_id;
    let change = batch(library, uuid::Uuid::new_v4(), "must be locked");
    let error = LibraryDurabilityPort::commit(&plane, root, &change).unwrap_err();
    assert_eq!(error.code(), "io");
}

#[test]
fn library_durability_domain_rejects_unsafe_resources_and_duplicate_typed_targets() {
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let valid = batch(library, uuid::Uuid::new_v4(), "validation");
    let mut duplicate = valid.clone();
    duplicate.records.push(duplicate.records[0].clone());
    assert!(duplicate.validate().is_err());
    for path in [
        "../escape",
        "/absolute",
        "echo/manifest.json",
        "Echo/transactions/not-media",
        "echo/assets/wrong",
        "echo/tmp/staging",
    ] {
        let mut invalid = valid.clone();
        invalid.resources.push(DurableResource {
            relative_path: path.to_owned(),
            content_hash: "a".repeat(64),
            byte_length: 1,
        });
        assert!(invalid.validate().is_err(), "unsafe resource {path}");
    }
    let mut duplicates = valid.clone();
    duplicates.resources = vec![
        DurableResource {
            relative_path: "media/test.flac".to_owned(),
            content_hash: "a".repeat(64),
            byte_length: 1
        };
        2
    ];
    assert!(duplicates.validate().is_err());
    let mut ambiguous = valid;
    ambiguous
        .expected
        .push(crate::domain::library::durability::ExpectedRecord {
            kind: RecordKind::Tombstone,
            object_uuid: uuid::Uuid::new_v4(),
            content_hash: None,
        });
    assert!(ambiguous.validate().is_err());
}

#[test]
fn library_durability_compact_typed_tombstones_allow_reused_playlist_name() {
    let (dir, registry, root, library, plane) = fixture();
    let old_id = uuid::Uuid::new_v4();
    let new_id = uuid::Uuid::new_v4();
    commit(&registry, root, &batch(library, old_id, "reused name")).unwrap();
    let database_dir = tempfile::tempdir().unwrap();
    let old =
        crate::infrastructure::sqlite::SqliteDatabase::open(database_dir.path().join("old.db"))
            .unwrap();
    LibraryRepository::upsert(
        &old,
        &LibraryRoot::new(root, dir.path().to_owned(), true, true),
    )
    .unwrap();
    old.configure_library_control(Arc::new(plane));
    old.project_library_commits(root, &recover(&registry, root).unwrap())
        .unwrap();
    let deleted = PortableRecordEnvelope::new(
        PortableRecord::Tombstone(crate::domain::library::TombstoneRecord {
            object_uuid: old_id,
            deleted_kind: RecordKind::Playlist,
            revision: Revision::from_u64(2),
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
        }),
        None,
    )
    .unwrap();
    let deletion = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![deleted],
    );
    commit(&registry, root, &deletion).unwrap();
    commit(&registry, root, &batch(library, new_id, "reused name")).unwrap();
    let replay = recover(&registry, root).unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let compacted = recover(&registry, root).unwrap();
    assert_eq!(compacted.checkpoint.as_ref().unwrap().records.len(), 2);
    old.project_library_commits(root, &compacted).unwrap();
    assert_eq!(
        PlaylistRepository::list(&old, root).unwrap(),
        vec![PlaylistId::from_uuid(new_id)]
    );
    assert!(
        read_operation(&registry, root, deletion.operation_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn library_durability_actual_record_rename_failure_remains_saved_and_retryable() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "native rename failure");
    let relative = portable::record_relative(change.records[0].record()).unwrap();
    let blocked = dir.path().join(relative);
    let error = commit_with_hook(&registry, root, &change, &mut |phase, _| {
        if phase == "record" {
            std::fs::create_dir_all(&blocked).unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), "library_saved_pending_recovery");
    assert!(marker_present(dir.path(), change.operation_id));
    std::fs::remove_dir(blocked).unwrap();
    LibraryDurabilityPort::commit(&plane, root, &change).unwrap();
    assert_eq!(chain(&registry, root).unwrap().commits[0].batch, change);
}
