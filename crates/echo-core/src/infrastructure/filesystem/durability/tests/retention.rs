//! Reclamation safety limits and the reference graph that protects unique bytes.
use super::*;
use std::collections::BTreeSet;

/// Every regular file below `echo/`, relative to the library root.
fn tree(directory: &std::path::Path) -> BTreeSet<String> {
    fn walk(base: &std::path::Path, path: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(relative) = path.strip_prefix(base) {
                out.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut files = BTreeSet::new();
    walk(directory, &directory.join("echo"), &mut files);
    files
}

fn under(files: &BTreeSet<String>, prefix: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|path| path.starts_with(prefix))
        .cloned()
        .collect()
}

#[test]
fn library_durability_active_transaction_limit_refuses_writes_without_removing_bytes() {
    let (directory, registry, root, library, _) = fixture();
    let saved = batch(library, uuid::Uuid::new_v4(), "saved before the limit");
    commit(&registry, root, &saved).unwrap();
    let folder = directory.path().join("echo/transactions");
    // Uncommitted crash evidence still occupies a live journal directory, so it
    // counts toward the safety limit exactly like a committed transaction.
    let mut extra = Vec::new();
    for _ in 0..MAX_ACTIVE {
        let id = uuid::Uuid::new_v4();
        std::fs::create_dir(folder.join(id.to_string())).unwrap();
        std::fs::write(folder.join(id.to_string()).join("prepared.json"), b"{}").unwrap();
        extra.push(id);
    }
    let error = chain(&registry, root).unwrap_err();
    match &error {
        Error::Conflict { what, .. } => {
            assert!(
                what.contains("active transaction safety limit exceeded"),
                "{what}"
            );
        }
        other => panic!("unexpected refusal: {other:?}"),
    }
    let refused = batch(library, uuid::Uuid::new_v4(), "must not be published");
    assert!(commit(&registry, root, &refused).is_err());
    assert!(!marker_present(directory.path(), refused.operation_id));
    assert!(!directory
        .path()
        .join(relative(refused.operation_id, "prepared.json"))
        .exists());
    // Refusing is not collecting: the saved journal and its payload stay intact.
    assert!(marker_present(directory.path(), saved.operation_id));
    assert!(directory
        .path()
        .join(relative(saved.operation_id, "prepared.json"))
        .is_file());
    for id in extra {
        std::fs::remove_dir_all(folder.join(id.to_string())).unwrap();
    }
    // Removing the crash evidence restores normal writes under the same identity.
    assert_eq!(recover(&registry, root).unwrap().head().unwrap().0, 1);
    assert_eq!(
        commit(&registry, root, &refused)
            .unwrap()
            .library_commit_seq,
        2
    );
}

#[test]
fn library_durability_interrupted_between_prepared_and_marker_removal_keeps_the_receipt() {
    for stop in [0, 1] {
        let (directory, registry, root, library, plane) = fixture();
        let bytes = b"referenced bytes that must survive every collection";
        let hash = LibraryDurabilityPort::write_asset(&plane, root, bytes).unwrap();
        let changes: Vec<_> = (0..3_u128)
            .map(|index| {
                let mut change = batch(library, uuid::Uuid::from_u128(0x10 + index), "covered");
                change.state.push(DurableStateChange::new(
                    StateKind::Pending,
                    uuid::Uuid::from_u128(0x100 + index),
                    serde_json::json!({"original_resource": hash, "index": index}),
                ));
                change.resources.push(DurableResource {
                    relative_path: format!("echo/assets/{hash}"),
                    content_hash: hash.clone(),
                    byte_length: bytes.len() as u64,
                });
                commit(&registry, root, &change).unwrap();
                change
            })
            .collect();
        let cache = ValidationCache::default();
        checkpoint::compact(
            &registry,
            &cache,
            root,
            &chain(&registry, root).unwrap(),
            &mut |phase, index| {
                if phase == "gc:remove" && index == stop {
                    Err(fail())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        // The hook fires after the named removal: index 0 leaves a marker whose
        // prepared body is already gone, index 1 leaves an empty journal folder.
        let mut orphaned = 0;
        for change in &changes {
            let prepared = directory
                .path()
                .join(relative(change.operation_id, "prepared.json"))
                .exists();
            let marker = marker_present(directory.path(), change.operation_id);
            assert!(!prepared || marker, "a marker is only removed after redo");
            orphaned += usize::from(marker && !prepared);
            // The immutable receipt alone still proves the frozen original,
            // including every resource declaration it carried.
            assert_eq!(
                read_operation(&registry, root, change.operation_id)
                    .unwrap()
                    .unwrap()
                    .batch,
                *change
            );
        }
        assert_eq!(orphaned, usize::from(stop == 0));
        // Restart finishes the interrupted collection from the durable anchor.
        let replay = recover(&registry, root).unwrap();
        assert_eq!(replay.head().unwrap().0, 3);
        assert_eq!(
            std::fs::read(directory.path().join(format!("echo/assets/{hash}"))).unwrap(),
            bytes
        );
        for change in &changes {
            assert_eq!(
                read_operation(&registry, root, change.operation_id)
                    .unwrap()
                    .unwrap()
                    .batch,
                *change
            );
        }
        assert_eq!(
            std::fs::read_dir(directory.path().join("echo/transactions"))
                .unwrap()
                .count(),
            0
        );
    }
}

/// Declare a media version, an immutable asset and an unfinished publication,
/// commit them, then add more than `WINDOW` commits so compaction must collect.
fn seeded_collection() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    RootControlPlane,
    LibraryBatch,
    String,
) {
    let (directory, registry, root, library, plane) = fixture();
    // A sync-managed immutable version and a user original, both referenced.
    let media = "echo/media/song/aa/declaration/cover.flac";
    let media_bytes = b"managed remote version that a fresh rebuild still needs";
    std::fs::create_dir_all(directory.path().join("echo/media/song/aa/declaration")).unwrap();
    std::fs::write(directory.path().join(media), media_bytes).unwrap();
    let asset_bytes = b"user cover original";
    let hash = LibraryDurabilityPort::write_asset(&plane, root, asset_bytes).unwrap();
    // Staging bytes belong to an unfinished publication and are not declared.
    let staging = "echo/tmp/unfinished-operation/0";
    std::fs::create_dir_all(directory.path().join("echo/tmp/unfinished-operation")).unwrap();
    std::fs::write(directory.path().join(staging), b"staging").unwrap();
    let mut change = batch(library, uuid::Uuid::new_v4(), "declaring holder");
    for kind in [
        StateKind::Pending,
        StateKind::Conflict,
        StateKind::DeleteIntent,
        StateKind::ResourceLocation,
        StateKind::Receipt,
    ] {
        change.state.push(DurableStateChange::new(
            kind,
            uuid::Uuid::new_v4(),
            serde_json::json!({"original_resource": media, "asset": hash}),
        ));
    }
    change.resources.push(DurableResource {
        relative_path: media.to_owned(),
        content_hash: blake3::hash(media_bytes).to_hex().to_string(),
        byte_length: media_bytes.len() as u64,
    });
    change.resources.push(DurableResource {
        relative_path: format!("echo/assets/{hash}"),
        content_hash: hash.clone(),
        byte_length: asset_bytes.len() as u64,
    });
    commit(&registry, root, &change).unwrap();
    for index in 0..WINDOW {
        let mut later = batch(library, uuid::Uuid::new_v4(), &format!("later {index}"));
        later.state.push(DurableStateChange::new(
            StateKind::Claim,
            uuid::Uuid::new_v4(),
            serde_json::json!({"active": index}),
        ));
        commit(&registry, root, &later).unwrap();
    }
    (directory, registry, root, plane, change, hash)
}

/// A database built only from the post-collection directory image still knows
/// both declared resource positions, and never treats staging as a source.
fn assert_fresh_rebuild_keeps(
    directory: &std::path::Path,
    root: LibraryRootId,
    plane: RootControlPlane,
    replay: &LibraryReplay,
    declared: [&str; 2],
) {
    let database_dir = tempfile::tempdir().unwrap();
    let fresh =
        crate::infrastructure::sqlite::SqliteDatabase::open(database_dir.path().join("fresh.db"))
            .unwrap();
    LibraryRepository::upsert(
        &fresh,
        &LibraryRoot::new(root, directory.to_owned(), true, true),
    )
    .unwrap();
    fresh.configure_library_control(Arc::new(plane));
    fresh.project_library_commits(root, replay).unwrap();
    let rebuilt = fresh.read_recovery_records(root).unwrap();
    for path in declared {
        assert!(
            rebuilt
                .resources
                .iter()
                .any(|resource| resource.relative_path == path),
            "fresh SQLite lost declaration {path}"
        );
    }
    // Staging is never offered as a song source even though its bytes survived.
    assert!(rebuilt
        .resources
        .iter()
        .all(|resource| !resource.relative_path.starts_with("echo/tmp/")));
}

#[test]
fn library_durability_collection_never_removes_media_assets_or_staging_bytes() {
    let (directory, registry, root, plane, change, hash) = seeded_collection();
    let media = "echo/media/song/aa/declaration/cover.flac";
    let media_bytes = b"managed remote version that a fresh rebuild still needs";
    let asset_bytes = b"user cover original";
    let staging = "echo/tmp/unfinished-operation/0";
    let before = tree(directory.path());
    let replay = recover(&registry, root).unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let collected = recover(&registry, root).unwrap();
    // Collection really happened: the covered journals are gone, which is what
    // makes the byte comparison below meaningful instead of vacuous.
    assert_eq!(
        std::fs::read_dir(directory.path().join("echo/transactions"))
            .map_or(0, std::iter::Iterator::count),
        0
    );
    assert!(collected.checkpoint.is_some());
    // Every candidate, conflict, deletion, location and publication byte is held.
    assert_eq!(
        std::fs::read(directory.path().join(media)).unwrap(),
        media_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join(format!("echo/assets/{hash}"))).unwrap(),
        asset_bytes
    );
    assert!(directory.path().join(staging).is_file());
    // Reclamation only reached journal files and obsolete snapshot leaves.
    let after = tree(directory.path());
    for prefix in ["media/", "assets/", "tmp/"] {
        assert_eq!(
            under(&after, prefix),
            under(&before, prefix),
            "collection removed bytes under echo/{prefix}"
        );
    }
    // The coverage image carries both declarations forward after the journal is
    // gone, so a fresh projection still knows the managed version's hash/length.
    let asset_path = format!("echo/assets/{hash}");
    let declared = [media, asset_path.as_str()];
    let retained = collected.checkpoint.as_ref().unwrap();
    for path in declared {
        assert!(
            retained
                .resources
                .iter()
                .any(|resource| resource.relative_path == path),
            "declaration {path} left the recoverable image"
        );
    }
    // Candidates, conflicts, delete intents and unfinished publications are
    // referenced evidence: every one of them survives the collection.
    for state in &change.state {
        assert!(
            retained.state.iter().any(|kept| kept == state),
            "referenced {} evidence was collected",
            state.kind.directory()
        );
    }
    assert_eq!(
        read_operation(&registry, root, change.operation_id)
            .unwrap()
            .unwrap()
            .batch,
        change
    );
    assert_fresh_rebuild_keeps(directory.path(), root, plane, &collected, declared);
}
