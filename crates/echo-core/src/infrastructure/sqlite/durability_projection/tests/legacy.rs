use super::*;
use crate::application::continuation::ContinueFromRecords;
use crate::application::ports::{ControlPlanePort, LibraryDurabilityPort};
use crate::application::testkit::small_fakes::MemoryControlPlane;
use crate::domain::library::{LibraryManifest, TombstoneRecord};

fn continue_records(
    control: &dyn ControlPlanePort,
    db: &SqliteDatabase,
    root: LibraryRootId,
) -> crate::application::continuation::ContinuationReport {
    ContinueFromRecords::new(control, db, db, db)
        .with_projection(db)
        .run(root)
        .unwrap()
}

#[test]
fn continuation_reads_real_v2_envelopes_without_rewriting_them_or_creating_uploads() {
    let (directory, db, root) = database();
    let path = directory.path().join("library");
    std::fs::create_dir_all(&path).unwrap();
    let registry = crate::infrastructure::filesystem::RootRegistry::new();
    registry.register(root, path);
    let control = crate::infrastructure::filesystem::RootControlPlane::new(registry);
    let mut manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test");
    manifest.format_version = 2;
    control.write_manifest(root, &manifest).unwrap();
    let envelope = PortableRecordEnvelope::new(song(), None).unwrap();
    control.write_record_envelope(root, &envelope).unwrap();
    let before = control
        .list_record_documents(root, crate::domain::library::RecordKind::Song)
        .unwrap();

    assert_eq!(continue_records(&control, &db, root).projection.songs, 1);
    assert_eq!(
        control
            .list_record_documents(root, crate::domain::library::RecordKind::Song)
            .unwrap(),
        before
    );
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        0
    );
    db.with_reader(|c| {
        let uploads: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_mutations", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(uploads, 0);
        Ok(())
    })
    .unwrap();
}

fn song() -> PortableRecord {
    Issue72Fixture::with_database_evidence()
        .database
        .into_iter()
        .find(|record| matches!(record, PortableRecord::Song(_)))
        .unwrap()
}

#[test]
fn continuation_legacy_projection_retains_orphans_without_commits_or_upload_echo() {
    let (_directory, db, root) = database();
    let records: Vec<_> = Issue72Fixture::with_database_evidence()
        .database
        .into_iter()
        .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
        .collect();
    let (songs, relations): (Vec<_>, Vec<_>) = records
        .iter()
        .cloned()
        .partition(|record| matches!(record.record(), PortableRecord::Song(_)));

    db.project_legacy_records(root, &relations).unwrap();
    assert!(!db.projection_status(root).unwrap().unresolved.is_empty());
    assert!(db.all_in_root(root).unwrap().is_empty());
    db.project_legacy_records(root, &songs).unwrap();
    assert_eq!(db.all_in_root(root).unwrap().len(), 15);
    assert!(db.projection_status(root).unwrap().unresolved.is_empty());
    let before = db.read_recovery_records(root).unwrap();
    db.project_legacy_records(root, &records).unwrap();
    CanonicalLibraryState::from_parts(&before.records, &before.state, &before.resources)
        .assert_matches(&CanonicalLibraryState::from_parts(
            &db.read_recovery_records(root).unwrap().records,
            &[],
            &[],
        ));

    let status = db.projection_status(root).unwrap();
    assert_eq!(status.applied_library_commit_seq, 0);
    assert_eq!(status.applied_head_digest, None);
    db.with_reader(|c| {
        for table in [
            "library_projection_watermarks",
            "library_applied_operations",
            "sync_mutations",
            "sync_outbox",
        ] {
            let count: u64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .map_err(storage)?;
            assert_eq!(count, 0, "legacy projection must not write {table}");
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn continuation_ordered_projection_prefers_committed_values_over_stale_record_snapshots() {
    let (_directory, db, root) = database();
    let control = MemoryControlPlane::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let stale = song();
    let mut current = stale.clone();
    if let PortableRecord::Song(record) = &mut current {
        record.title = Some("committed directory value".to_owned());
    }
    for record in [&stale, &current] {
        control
            .commit(
                root,
                &LibraryBatch::new(
                    uuid::Uuid::new_v4(),
                    library,
                    ChangeOrigin::Scan,
                    vec![PortableRecordEnvelope::new(record.clone(), None).unwrap()],
                ),
            )
            .unwrap();
    }
    control.write_record(root, &stale).unwrap();
    let mut legacy = stale;
    if let PortableRecord::Song(record) = &mut legacy {
        record.song_uuid = uuid::Uuid::new_v4();
        record.media_path = LibraryRelativePath::new("media/legacy.flac").unwrap();
        record.title = Some("unjournaled legacy song".to_owned());
    }
    control.write_record(root, &legacy).unwrap();
    let before = control.records_of(root);
    let report = continue_records(&control, &db, root);
    assert_eq!(report.projection.songs, 2);
    let projected = SongRepository::by_id(&db, SongId::from_uuid(current.object_uuid()))
        .unwrap()
        .unwrap();
    assert_eq!(projected.title(), Some("committed directory value"));
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        2
    );
    continue_records(&control, &db, root);
    assert_eq!(
        control.records_of(root),
        before,
        "compatibility reads do not rewrite records"
    );
    assert_eq!(control.recover(root).unwrap().commits.len(), 2);
    db.with_reader(|c| {
        let uploads: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_mutations", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(uploads, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn continuation_legacy_newer_song_outranks_older_typed_tombstone_without_rewriting_source() {
    let (_directory, db, root) = database();
    let control = MemoryControlPlane::new();
    let mut manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test");
    manifest.format_version = 2;
    control.write_manifest(root, &manifest).unwrap();
    let record = song();
    control.write_record(root, &record).unwrap();
    control
        .write_record(
            root,
            &PortableRecord::Tombstone(TombstoneRecord {
                object_uuid: record.object_uuid(),
                deleted_kind: crate::domain::library::RecordKind::Song,
                revision: Revision::from_u64(99),
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::default(),
            }),
        )
        .unwrap();
    let before = control.records_of(root);
    let report = continue_records(&control, &db, root);
    assert_eq!(report.projection.songs, 1);
    assert_eq!(report.projection.tombstoned, 0);
    assert!(
        SongRepository::by_id(&db, SongId::from_uuid(record.object_uuid()))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        0
    );
    assert_eq!(control.records_of(root), before);
}

#[test]
fn continuation_legacy_snapshot_can_replace_its_own_temporary_tombstone_on_a_later_read() {
    let (_directory, db, root) = database();
    let control = MemoryControlPlane::new();
    let mut manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test");
    manifest.format_version = 2;
    control.write_manifest(root, &manifest).unwrap();
    let mut record = song();
    let id = SongId::from_uuid(record.object_uuid());
    control.write_record(root, &record).unwrap();
    continue_records(&control, &db, root);
    let deleted_at = record.hlc().wall_secs + 1;
    control
        .write_record(
            root,
            &PortableRecord::Tombstone(TombstoneRecord {
                object_uuid: id.as_uuid(),
                deleted_kind: crate::domain::library::RecordKind::Song,
                revision: Revision::from_u64(99),
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::new(deleted_at, 0),
            }),
        )
        .unwrap();
    continue_records(&control, &db, root);
    assert_eq!(
        SongRepository::by_id(&db, id)
            .unwrap()
            .unwrap()
            .availability(),
        crate::domain::entities::SongAvailability::PendingDelete
    );
    if let PortableRecord::Song(song) = &mut record {
        song.hlc = HybridLogicalClock::new(deleted_at + 1, 0);
    }
    control.write_record(root, &record).unwrap();
    continue_records(&control, &db, root);
    assert_eq!(
        SongRepository::by_id(&db, id)
            .unwrap()
            .unwrap()
            .availability(),
        crate::domain::entities::SongAvailability::Missing
    );
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        0
    );
}

#[test]
fn continuation_committed_tombstone_cannot_be_replaced_by_a_newer_legacy_snapshot() {
    let (_directory, db, root) = database();
    let control = MemoryControlPlane::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let record = song();
    let id = SongId::from_uuid(record.object_uuid());
    for image in [
        record.clone(),
        PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: id.as_uuid(),
            deleted_kind: crate::domain::library::RecordKind::Song,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
        }),
    ] {
        control
            .commit(
                root,
                &LibraryBatch::new(
                    uuid::Uuid::new_v4(),
                    library,
                    ChangeOrigin::LocalEdit,
                    vec![PortableRecordEnvelope::new(image, None).unwrap()],
                ),
            )
            .unwrap();
    }
    control.write_record(root, &record).unwrap();
    let report = continue_records(&control, &db, root);
    assert_eq!(report.projection.tombstoned, 1);
    assert_eq!(
        SongRepository::by_id(&db, id)
            .unwrap()
            .unwrap()
            .availability(),
        crate::domain::entities::SongAvailability::PendingDelete
    );
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        2
    );
}
