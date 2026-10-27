//! Native temporary-directory tests for the library directory protocol.
use super::*;
use crate::application::ports::{
    ControlPlanePort, LibraryDurabilityPort, LibraryProjectionPort, LibraryRepository,
    PlaylistRepository,
};
use crate::domain::entities::LibraryRoot;
use crate::domain::ids::{PlaylistId, Revision};
use crate::domain::library::durability::{
    ChangeOrigin, DurableResource, DurableStateChange, StateKind,
};
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryId, PlaylistRecord, PortableRecord,
    PortableRecordEnvelope, RecordKind,
};
use crate::infrastructure::filesystem::RootControlPlane;
use std::sync::Arc;

// These protocol fault tests intentionally exercise cold intrinsic validation.
fn chain(registry: &RootRegistry, root: LibraryRootId) -> Result<LibraryReplay, Error> {
    super::chain(registry, &ValidationCache::default(), root)
}
fn recover(registry: &RootRegistry, root: LibraryRootId) -> Result<LibraryReplay, Error> {
    super::recover(registry, &ValidationCache::default(), root)
}
fn recover_with_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<LibraryReplay, Error> {
    super::recover_with_hook(registry, &ValidationCache::default(), root, hook)
}
fn commit(
    registry: &RootRegistry,
    root: LibraryRootId,
    batch: &LibraryBatch,
) -> Result<CommitOutcome, Error> {
    super::commit(registry, &ValidationCache::default(), root, batch)
}
fn commit_with_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    batch: &LibraryBatch,
    hook: &mut Hook<'_>,
) -> Result<CommitOutcome, Error> {
    super::commit_with_hook(registry, &ValidationCache::default(), root, batch, hook)
}
fn read_operation(
    registry: &RootRegistry,
    root: LibraryRootId,
    id: uuid::Uuid,
) -> Result<Option<LibraryCommit>, Error> {
    super::read_operation(registry, &ValidationCache::default(), root, id)
}
fn operation_status(
    registry: &RootRegistry,
    root: LibraryRootId,
    id: uuid::Uuid,
) -> Result<Option<CommitOutcome>, Error> {
    super::operation_status(registry, &ValidationCache::default(), root, id)
}

fn fixture() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    LibraryId,
    RootControlPlane,
) {
    let dir = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, dir.path());
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let plane = RootControlPlane::new(registry.clone());
    plane
        .write_manifest(root, &LibraryManifest::new(library, "durability-test"))
        .unwrap();
    (dir, registry, root, library, plane)
}
fn batch(library: LibraryId, id: uuid::Uuid, title: &str) -> LibraryBatch {
    LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![
            PortableRecordEnvelope::new(
                PortableRecord::Playlist(PlaylistRecord {
                    playlist_uuid: id,
                    revision: Revision::INITIAL,
                    updated_by_device_id: DeviceId::new(),
                    hlc: HybridLogicalClock::default(),
                    display_name: title.to_owned(),
                    created_at: None,
                    cover_asset: None,
                }),
                None,
            )
            .unwrap(),
        ],
    )
}
fn marker_present(dir: &std::path::Path, id: uuid::Uuid) -> bool {
    dir.join(relative(id, "committed.json")).is_file()
}
fn fail() -> Error {
    invalid("injected native boundary failure")
}

fn resource_and_state(registry: &RootRegistry, root: LibraryRootId, change: &mut LibraryBatch) {
    let bytes = b"fault-tested original";
    let hash = write_asset(registry, root, bytes).unwrap();
    change.resources.push(DurableResource {
        relative_path: format!("echo/assets/{hash}"),
        content_hash: hash,
        byte_length: bytes.len() as u64,
    });
    change.state.push(DurableStateChange::new(
        StateKind::Claim,
        uuid::Uuid::new_v4(),
        serde_json::json!({"active": true}),
    ));
}

#[test]
fn library_durability_fs_commit_retry_and_payload_identity() {
    let (_dir, registry, root, library, plane) = fixture();
    let batch = batch(library, uuid::Uuid::new_v4(), "保存");
    let first = LibraryDurabilityPort::commit(&plane, root, &batch).unwrap();
    assert_eq!(first.library_commit_seq, 1);
    assert_eq!(
        LibraryDurabilityPort::commit(&plane, root, &batch).unwrap(),
        first
    );
    let mut different = batch;
    different.origin = ChangeOrigin::Import;
    assert!(LibraryDurabilityPort::commit(&plane, root, &different).is_err());
    assert_eq!(chain(&registry, root).unwrap().commits.len(), 1);
}

#[test]
fn library_durability_every_native_commit_boundary_recovers_same_operation() {
    let (_dir, registry, root, library, _) = fixture();
    let mut prototype = batch(library, uuid::Uuid::new_v4(), "故障");
    resource_and_state(&registry, root, &mut prototype);
    let mut trace = Vec::new();
    commit_with_hook(&registry, root, &prototype, &mut |phase, index| {
        trace.push((phase.to_owned(), index));
        Ok(())
    })
    .unwrap();
    assert!(trace.iter().any(|(p, _)| p == "marker:directory_sync"));
    assert!(trace.iter().any(|(p, _)| p == "checkpoint:rename"));
    assert!(trace.iter().any(|(p, _)| p == "resource:file_sync"));
    assert!(trace.iter().any(|(p, _)| p == "state:rename"));
    for (stop, boundary) in trace.iter().enumerate() {
        let (dir, registry, root, library, plane) = fixture();
        let mut batch = batch(library, uuid::Uuid::new_v4(), "故障");
        resource_and_state(&registry, root, &mut batch);
        let mut counter = 0;
        let result = commit_with_hook(&registry, root, &batch, &mut |_, _| {
            let fail_here = counter == stop;
            counter += 1;
            if fail_here { Err(fail()) } else { Ok(()) }
        });
        assert!(result.is_err(), "boundary {stop}: {boundary:?}");
        let committed = marker_present(dir.path(), batch.operation_id);
        assert_eq!(
            result.unwrap_err().code() == "library_saved_pending_recovery",
            committed,
            "boundary {stop}: {boundary:?}"
        );
        let status = operation_status(&registry, root, batch.operation_id).unwrap();
        if let Some(status) = status {
            assert_ne!(status.status == CommitStatus::NotSaved, committed);
        }
        if !committed {
            assert!(chain(&registry, root).unwrap().commits.is_empty());
        }
        LibraryDurabilityPort::commit(&plane, root, &batch).unwrap();
        let replay = recover(&registry, root).unwrap();
        assert_eq!(replay.commits.len(), 1, "boundary {stop}");
        assert_eq!(replay.commits[0].batch, batch);
        assert_eq!(replay.commits[0].library_commit_seq, 1);
    }
}

#[test]
fn library_durability_prepared_retry_rebases_only_uncommitted_sequence() {
    let (_dir, registry, root, library, _) = fixture();
    let pending = batch(library, uuid::Uuid::new_v4(), "first preparation");
    commit_with_hook(&registry, root, &pending, &mut |phase, _| {
        if phase == "prepared" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let other = batch(library, uuid::Uuid::new_v4(), "other operation");
    commit(&registry, root, &other).unwrap();
    assert_eq!(
        commit(&registry, root, &pending)
            .unwrap()
            .library_commit_seq,
        2
    );
    assert_eq!(
        read_operation(&registry, root, pending.operation_id)
            .unwrap()
            .unwrap()
            .batch,
        pending
    );
}

#[test]
fn library_durability_c1_failure_then_c2_never_rolls_back_latest_record() {
    let (_dir, registry, root, library, plane) = fixture();
    let id = uuid::Uuid::new_v4();
    let first = batch(library, id, "C1");
    commit_with_hook(&registry, root, &first, &mut |p, _| {
        if p == "record" { Err(fail()) } else { Ok(()) }
    })
    .unwrap_err();
    let second = batch(library, id, "C2");
    commit(&registry, root, &second).unwrap();
    for _ in 0..3 {
        recover(&registry, root).unwrap();
    }
    let actual = plane
        .read_record(root, RecordKind::Playlist, &id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(&actual, second.records[0].record());
}

#[test]
fn library_durability_read_paths_do_not_create_or_acquire_write_lock() {
    let (dir, registry, root, library, plane) = fixture();
    let batch = batch(library, uuid::Uuid::new_v4(), "浏览");
    commit(&registry, root, &batch).unwrap();
    let hash = LibraryDurabilityPort::write_asset(&plane, root, b"original cover").unwrap();
    let lock = dir.path().join("echo/write.lock");
    std::fs::remove_file(&lock).unwrap();
    assert!(plane.read_manifest(root).unwrap().is_some());
    assert_eq!(
        plane
            .list_record_documents(root, RecordKind::Playlist)
            .unwrap()
            .len(),
        1
    );
    assert!(
        LibraryDurabilityPort::read_asset(&plane, root, &hash)
            .unwrap()
            .is_some()
    );
    assert!(
        LibraryDurabilityPort::operation_status(&plane, root, batch.operation_id)
            .unwrap()
            .is_some()
    );
    assert!(!lock.exists(), "reads must not require write.lock");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&file).unwrap();
    let independent = RootControlPlane::new(registry);
    assert_eq!(
        independent
            .list_records(root, RecordKind::Playlist)
            .unwrap()
            .len(),
        1
    );
    assert!(LibraryDurabilityPort::commit(&independent, root, &batch).is_err());
}

#[cfg(unix)]
#[test]
fn library_durability_readonly_volume_and_unmaterialized_commit_can_be_browsed() {
    use std::os::unix::fs::PermissionsExt as _;
    let (dir, registry, root, library, plane) = fixture();
    let batch = batch(library, uuid::Uuid::new_v4(), "只读");
    commit_with_hook(&registry, root, &batch, &mut |phase, _| {
        if phase == "committed" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let lock = dir.path().join("echo/write.lock");
    std::fs::remove_file(&lock).unwrap();
    let echo = dir.path().join("echo");
    std::fs::set_permissions(&echo, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        plane
            .list_records(root, RecordKind::Playlist)
            .unwrap()
            .len(),
        1
    );
    assert!(
        plane
            .read_record_document(
                root,
                RecordKind::Playlist,
                &batch.records[0].record().object_uuid().to_string()
            )
            .unwrap()
            .is_some()
    );
    assert!(!lock.exists());
    std::fs::set_permissions(echo, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn library_durability_compaction_bounds_active_chain_and_preserves_old_retries() {
    let (dir, registry, root, library, plane) = fixture();
    let id = uuid::Uuid::new_v4();
    let first = batch(library, id, "0");
    commit(&registry, root, &first).unwrap();
    for i in 1..WINDOW * 3 + 5 {
        commit(&registry, root, &batch(library, id, &i.to_string())).unwrap();
    }
    let replay = recover(&registry, root).unwrap();
    assert!(replay.commits.len() < WINDOW);
    assert_eq!(replay.checkpoint.as_ref().unwrap().records.len(), 1);
    assert!(
        std::fs::read_dir(dir.path().join("echo/transactions"))
            .unwrap()
            .count()
            < WINDOW
    );
    assert_eq!(
        commit(&registry, root, &first).unwrap().library_commit_seq,
        1
    );
    let mut changed = first.clone();
    changed.origin = ChangeOrigin::Remote;
    assert!(commit(&registry, root, &changed).is_err());
    assert_eq!(
        LibraryDurabilityPort::read_operation(&plane, root, first.operation_id)
            .unwrap()
            .unwrap()
            .batch,
        first
    );
    assert_eq!(replay.head().unwrap().0, (WINDOW * 3 + 5) as u64);
}

#[test]
fn library_durability_broken_chain_and_corrupt_checkpoint_fail_closed_preserve_bytes() {
    let (dir, registry, root, library, _) = fixture();
    let first = batch(library, uuid::Uuid::new_v4(), "1");
    commit(&registry, root, &first).unwrap();
    let second = batch(library, uuid::Uuid::new_v4(), "2");
    commit(&registry, root, &second).unwrap();
    let prepared = dir
        .path()
        .join(relative(second.operation_id, "prepared.json"));
    let original = std::fs::read(&prepared).unwrap();
    let mut commit: LibraryCommit = serde_json::from_slice(&original).unwrap();
    commit.predecessor_digest = None;
    commit.digest = LibraryCommit::calculate_digest(&commit.batch, 2, None).unwrap();
    let marker = Marker {
        version: 1,
        seq: 2,
        digest: commit.digest.clone(),
    };
    std::fs::write(&prepared, serde_json::to_vec(&commit).unwrap()).unwrap();
    std::fs::write(
        dir.path()
            .join(relative(second.operation_id, "committed.json")),
        serde_json::to_vec(&marker).unwrap(),
    )
    .unwrap();
    assert!(recover(&registry, root).is_err());
    let broken = std::fs::read(&prepared).unwrap();
    assert!(
        super::commit(
            &registry,
            &ValidationCache::default(),
            root,
            &batch(library, uuid::Uuid::new_v4(), "3")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&prepared).unwrap(), broken);
    std::fs::write(dir.path().join(CHECKPOINT), b"{corrupt evidence}").unwrap();
    assert!(recover(&registry, root).is_err());
    assert_eq!(
        std::fs::read(dir.path().join(CHECKPOINT)).unwrap(),
        b"{corrupt evidence}"
    );
}

#[test]
fn library_durability_empty_and_old_sqlite_watermarks_rebuild_after_compaction() {
    let (dir, registry, root, library, plane) = fixture();
    let id = uuid::Uuid::new_v4();
    let first = batch(library, id, "original");
    commit(&registry, root, &first).unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let old =
        crate::infrastructure::sqlite::SqliteDatabase::open(db_dir.path().join("old.db")).unwrap();
    LibraryRepository::upsert(
        &old,
        &LibraryRoot::new(root, dir.path().to_owned(), true, true),
    )
    .unwrap();
    old.configure_library_control(Arc::new(plane.clone()));
    old.project_library_commits(root, &recover(&registry, root).unwrap())
        .unwrap();
    for i in 1..WINDOW + 2 {
        commit(
            &registry,
            root,
            &batch(library, id, &format!("current {i}")),
        )
        .unwrap();
    }
    let replay = recover(&registry, root).unwrap();
    assert!(replay.checkpoint.is_some());
    old.project_library_commits(root, &replay).unwrap();
    let fresh = crate::infrastructure::sqlite::SqliteDatabase::open(db_dir.path().join("fresh.db"))
        .unwrap();
    LibraryRepository::upsert(
        &fresh,
        &LibraryRoot::new(root, dir.path().to_owned(), true, true),
    )
    .unwrap();
    fresh.configure_library_control(Arc::new(plane));
    fresh.project_library_commits(root, &replay).unwrap();
    let expected = Some(format!("current {}", WINDOW + 1));
    assert_eq!(
        PlaylistRepository::name(&old, PlaylistId::from_uuid(id)).unwrap(),
        expected
    );
    assert_eq!(
        PlaylistRepository::name(&fresh, PlaylistId::from_uuid(id)).unwrap(),
        expected
    );
    fresh.project_library_commits(root, &replay).unwrap();
}

mod faults;
mod identity;

mod playback_privacy;
mod publication;

mod validation_cache;

mod fences;

mod scoped_state;

mod gc_performance;
mod resources;
mod retention;
mod snapshot_gc;
