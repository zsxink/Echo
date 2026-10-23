//! Local directory ownership is independent of preserved sync binding evidence.
use super::*;
use crate::domain::library::{durability::ProjectionUnresolvedReason, PortableSyncMetadata};
use crate::domain::sync::{AuthorityId, Epoch, ObjectVersion, SyncBinding};
use crate::infrastructure::sqlite::SqliteDatabase;

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn library_durability_distinct_directory_and_sync_ids_survive_recovery_and_fresh_projection() {
    let (dir, registry, root, library, plane) = fixture();
    let previous = binding();
    let replacement = binding();
    assert_ne!(library.as_uuid(), previous.library_id.as_uuid());
    assert_ne!(previous.library_id, replacement.library_id);
    let playlist_id = uuid::Uuid::new_v4();
    let mut change = batch(library, playlist_id, "historical binding");
    change.records[0] = PortableRecordEnvelope::new(
        change.records[0].record().clone(),
        Some(PortableSyncMetadata {
            binding: previous,
            object_version: ObjectVersion::INITIAL,
            aliases: vec![],
        }),
    )
    .unwrap();
    let pending_id = uuid::Uuid::new_v4();
    let adoption_id = uuid::Uuid::new_v4();
    change.state = vec![
        DurableStateChange::new(
            StateKind::Pending,
            pending_id,
            serde_json::json!({"binding": previous, "mutation_id": pending_id}),
        ),
        DurableStateChange::new(
            StateKind::Adoption,
            adoption_id,
            serde_json::json!({
                "previous_binding": previous,
                "replacement_binding": replacement,
                "old_event": pending_id,
                "new_event": uuid::Uuid::new_v4(),
                "historical": [{"library_id": previous.library_id}],
                "diagnostic": {"library": "historical label"}
            }),
        ),
    ];
    let first = LibraryDurabilityPort::commit(&plane, root, &change).unwrap();
    assert_eq!(
        LibraryDurabilityPort::commit(&plane, root, &change).unwrap(),
        first
    );
    let mut rewritten = change.clone();
    rewritten.state[0].payload["binding"] = serde_json::to_value(replacement).unwrap();
    assert!(LibraryDurabilityPort::commit(&plane, root, &rewritten).is_err());
    assert_eq!(
        LibraryDurabilityPort::read_operation(&plane, root, change.operation_id)
            .unwrap()
            .unwrap()
            .batch,
        change
    );
    let replay = recover(&registry, root).unwrap();
    assert_eq!(replay.commits[0].batch, change);
    // A coverage image must preserve the same evidence and directory owner.
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let replay = recover(&registry, root).unwrap();
    let image = replay.checkpoint.as_ref().unwrap();
    assert_eq!(image.library_id, library);
    assert_eq!(image.records, change.records);
    assert_eq!(image.state.len(), change.state.len());
    for state in &change.state {
        assert!(image.state.contains(state));
    }
    let database_dir = tempfile::tempdir().unwrap();
    let database_path = database_dir.path().join("fresh.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, dir.path().to_owned(), true, true),
    )
    .unwrap();
    database.configure_library_control(Arc::new(plane));
    database.project_library_commits(root, &replay).unwrap();
    assert_eq!(
        PlaylistRepository::name(&database, PlaylistId::from_uuid(playlist_id)).unwrap(),
        Some("historical binding".to_owned())
    );
    let retained = database.read_recovery_records(root).unwrap();
    assert_eq!(retained.records, change.records);
    for state in &change.state {
        assert!(retained.state.contains(state));
    }
    let status = database.projection_status(root).unwrap();
    assert_eq!(status.applied_library_commit_seq, first.library_commit_seq);
    assert_eq!(status.unresolved.len(), 2);
    assert!(status
        .unresolved
        .iter()
        .all(|record| { record.reason == ProjectionUnresolvedReason::RequiresStateRecovery }));
    // Restoring old private evidence never installs credentials, a live binding,
    // or new upload candidates in the independently rebuilt database.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    for table in ["sync_binding", "sync_mutations", "sync_outbox"] {
        let count: u64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "historical evidence must not activate {table}");
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT library_id FROM library_projection_watermarks WHERE library_root_uuid=?1",
                [root.to_string()],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        library.to_string()
    );
    database.project_library_commits(root, &replay).unwrap();
    assert_eq!(database.read_recovery_records(root).unwrap(), retained);
}

#[test]
fn library_durability_rejects_foreign_directory_batch_chain_and_checkpoint() {
    let (dir, registry, root, library, plane) = fixture();
    let foreign = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let foreign_change = batch(foreign, uuid::Uuid::new_v4(), "foreign");
    assert!(LibraryDurabilityPort::commit(&plane, root, &foreign_change).is_err());
    assert!(!marker_present(dir.path(), foreign_change.operation_id));
    assert!(chain(&registry, root).unwrap().commits.is_empty());
    let change = batch(library, uuid::Uuid::new_v4(), "owned");
    commit(&registry, root, &change).unwrap();
    // A validly hashed foreign transaction is still rejected by the manifest.
    let mut wrong = read_operation(&registry, root, change.operation_id)
        .unwrap()
        .unwrap();
    wrong.batch.library_id = foreign;
    wrong.digest = LibraryCommit::calculate_digest(&wrong.batch, 1, None).unwrap();
    std::fs::write(
        dir.path()
            .join(relative(change.operation_id, "prepared.json")),
        serde_json::to_vec(&wrong).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path()
            .join(relative(change.operation_id, "committed.json")),
        serde_json::to_vec(&Marker {
            version: 1,
            seq: 1,
            digest: wrong.digest,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(recover(&registry, root).is_err());
    assert!(read_operation(&registry, root, change.operation_id).is_err());
    // A directory copied to another manifest owner cannot lend it its anchor.
    let (_dir, registry, root, library, plane) = fixture();
    commit(
        &registry,
        root,
        &batch(library, uuid::Uuid::new_v4(), "anchor"),
    )
    .unwrap();
    let replay = recover(&registry, root).unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    plane
        .write_manifest(root, &LibraryManifest::new(foreign, "other owner"))
        .unwrap();
    assert!(recover(&registry, root).is_err());
}
