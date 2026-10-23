//! Legacy format migration contracts and original-evidence preservation.
use super::*;
use crate::application::ports::{ControlPlanePort, ManifestState};
use crate::domain::ids::Revision;
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryId, LibraryRelativePath, SongRecord, TombstoneRecord,
};
use crate::infrastructure::filesystem::RootControlPlane;

fn fixture() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    LibraryManifest,
) {
    let dir = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, dir.path());
    let target = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "upgrade-test");
    let mut original = target.clone();
    original.format_version = 1;
    write(
        &registry,
        root,
        MANIFEST,
        &serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    (dir, registry, root, target)
}

fn song() -> PortableRecord {
    PortableRecord::Song(SongRecord {
        song_uuid: uuid::Uuid::new_v4(),
        revision: Revision::from_u64(7),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1234, 9),
        media_path: LibraryRelativePath::new("media/历史/一首歌.flac").unwrap(),
        content_hash: "old-content-hash".to_owned(),
        added_at: 1_234_567_890,
        title: Some("历史标题".to_owned()),
        artist: Some("作者".to_owned()),
        album: None,
    })
}

fn seed(registry: &RootRegistry, root: LibraryRootId, record: &PortableRecord) -> String {
    // Unknown fields and noncanonical whitespace must survive in the backup
    // byte for byte, independently from typed deserialization.
    let mut value = serde_json::to_value(record).unwrap();
    value["historical_extension"] = serde_json::json!({"未知": [1, "原字段"]});
    let raw = format!("\n{}\n", serde_json::to_string_pretty(&value).unwrap());
    write(
        registry,
        root,
        &original_relative(record.kind(), record.object_uuid(), None),
        raw.as_bytes(),
    )
    .unwrap();
    raw
}

#[test]
fn partial_upgrade_is_fenced_and_new_adapter_recovers_the_persisted_plan() {
    let (_dir, registry, root, target) = fixture();
    let records = [song(), song()];
    let originals: Vec<_> = records
        .iter()
        .map(|record| seed(&registry, root, record))
        .collect();
    let updates: Vec<_> = records
        .iter()
        .map(|record| PortableRecordEnvelope::new(record.clone(), None).unwrap())
        .collect();
    let error = apply_with_hook(&registry, root, &updates, &target, &mut |phase, index| {
        if phase == "record" && index == 1 {
            return Err(invalid("simulated disk failure"));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), "conflict");
    let manifest: LibraryManifest = decode(
        &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.format_version, FENCE_VERSION);
    // This is exactly the version check performed by the v1 open path.
    assert!(manifest.format_version > 1);
    let plan: Plan = decode(
        &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(plan.originals.len(), 2);
    for raw in &originals {
        assert!(plan.originals.iter().any(|original| &original.raw == raw));
    }
    let published: Vec<PortableRecordDocument> = records
        .iter()
        .map(|record| {
            decode(
                &read(
                    &registry,
                    root,
                    &record_relative(record).unwrap(),
                    MAX_RECORD_BYTES,
                )
                .unwrap()
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(
        published
            .iter()
            .filter(|document| matches!(document, PortableRecordDocument::V2(_)))
            .count(),
        1
    );
    // Reads never acquire a write lock or perform a format migration. A
    // writable activation explicitly resumes the persisted original plan.
    let restarted = RootControlPlane::new(registry.clone());
    assert!(restarted
        .list_record_documents(root, RecordKind::Song)
        .is_err());
    restarted
        .apply_portable_upgrade(root, &[], &target)
        .unwrap();
    let documents = restarted
        .list_record_documents(root, RecordKind::Song)
        .unwrap();
    assert_eq!(documents.len(), 2);
    assert!(documents
        .iter()
        .all(|document| matches!(document, PortableRecordDocument::V2(_))));
    for record in &records {
        assert!(documents.iter().any(|document| document.record() == record));
    }
    assert_eq!(restarted.read_manifest(root).unwrap(), Some(target));
    assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
    assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
        .unwrap()
        .is_some());
}

#[test]
fn journal_before_fence_is_recoverable_and_formal_manifest_is_last() {
    let (_dir, registry, root, target) = fixture();
    let record = song();
    seed(&registry, root, &record);
    let envelope = PortableRecordEnvelope::new(record.clone(), None).unwrap();
    apply_with_hook(&registry, root, &[envelope], &target, &mut |phase, _| {
        if phase == "journal" {
            return Err(invalid("crash before fence"));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(
        decode::<LibraryManifest>(
            &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .format_version,
        1
    );
    let backup: Plan = decode(
        &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    replay(&registry, root, &backup, &mut |phase, _| {
        if phase == "manifest" {
            let manifest: LibraryManifest =
                decode(&read(&registry, root, MANIFEST, MAX_RECORD_BYTES)?.unwrap())?;
            assert_eq!(manifest.format_version, FENCE_VERSION);
            assert!(matches!(
                decode::<PortableRecordDocument>(
                    &read(
                        &registry,
                        root,
                        &record_relative(&record)?,
                        MAX_RECORD_BYTES
                    )?
                    .unwrap()
                )?,
                PortableRecordDocument::V2(_)
            ));
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        RootControlPlane::new(registry).read_manifest(root).unwrap(),
        Some(target)
    );
}

#[test]
fn unmapped_original_aborts_before_any_journal_or_fence() {
    let (_dir, registry, root, target) = fixture();
    let record = song();
    let raw = seed(&registry, root, &record);
    assert!(apply(&registry, root, &[], &target).is_err());
    assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
        .unwrap()
        .is_none());
    assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
    assert_eq!(
        read(
            &registry,
            root,
            &record_relative(&record).unwrap(),
            MAX_RECORD_BYTES
        )
        .unwrap(),
        Some(raw)
    );
    assert_eq!(
        RootControlPlane::new(registry)
            .read_manifest(root)
            .unwrap()
            .unwrap()
            .format_version,
        1
    );
}

#[test]
fn corrupt_backup_stops_reads_and_writes_without_clearing_the_fence() {
    let (_dir, registry, root, target) = fixture();
    let record = song();
    seed(&registry, root, &record);
    apply_with_hook(
        &registry,
        root,
        &[PortableRecordEnvelope::new(record.clone(), None).unwrap()],
        &target,
        &mut |phase, _| {
            if phase == "fence" {
                return Err(invalid("crash"));
            }
            Ok(())
        },
    )
    .unwrap_err();
    write(&registry, root, BACKUP, b"{}").unwrap();
    let plane = RootControlPlane::new(registry.clone());
    assert!(plane.list_records(root, RecordKind::Song).is_err());
    assert!(plane.write_record(root, &record).is_err());
    assert!(plane
        .delete_record(root, RecordKind::Song, &record.object_uuid().to_string())
        .is_err());
    assert!(plane.write_manifest(root, &target).is_err());
    assert_eq!(
        decode::<LibraryManifest>(
            &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .format_version,
        FENCE_VERSION
    );
}

#[test]
fn typed_tombstones_with_the_same_uuid_coexist_and_delete_independently() {
    let (_dir, registry, root, target) = fixture();
    apply(&registry, root, &[], &target).unwrap();
    let plane = RootControlPlane::new(registry);
    let uuid = uuid::Uuid::new_v4();
    let tombstone = |kind| {
        PortableRecordEnvelope::new(
            PortableRecord::Tombstone(TombstoneRecord {
                object_uuid: uuid,
                deleted_kind: kind,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::default(),
            }),
            None,
        )
        .unwrap()
    };
    let song = tombstone(RecordKind::Song);
    let favorite = tombstone(RecordKind::Favorite);
    plane.write_record_envelope(root, &song).unwrap();
    plane.write_record_envelope(root, &favorite).unwrap();
    assert_eq!(
        plane
            .list_record_documents(root, RecordKind::Tombstone)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        plane
            .read_tombstone_document(root, RecordKind::Song, &uuid.to_string())
            .unwrap(),
        Some(PortableRecordDocument::V2(song))
    );
    assert!(plane
        .read_record(root, RecordKind::Tombstone, &uuid.to_string())
        .is_err());
    assert!(plane
        .delete_record(root, RecordKind::Tombstone, &uuid.to_string())
        .is_err());
    plane
        .delete_tombstone(root, RecordKind::Song, &uuid.to_string())
        .unwrap();
    assert_eq!(
        plane
            .read_tombstone_document(root, RecordKind::Favorite, &uuid.to_string())
            .unwrap(),
        Some(PortableRecordDocument::V2(favorite))
    );
}

#[test]
fn migration_moves_flat_tombstones_only_after_an_equivalent_typed_write() {
    let (_dir, registry, root, target) = fixture();
    let record = PortableRecord::Tombstone(TombstoneRecord {
        object_uuid: uuid::Uuid::new_v4(),
        deleted_kind: RecordKind::Favorite,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::default(),
    });
    let raw = seed(&registry, root, &record);
    let flat = original_relative(record.kind(), record.object_uuid(), None);
    apply(
        &registry,
        root,
        &[PortableRecordEnvelope::new(record.clone(), None).unwrap()],
        &target,
    )
    .unwrap();
    assert!(read(&registry, root, &flat, MAX_RECORD_BYTES)
        .unwrap()
        .is_none());
    assert!(read(
        &registry,
        root,
        &record_relative(&record).unwrap(),
        MAX_RECORD_BYTES
    )
    .unwrap()
    .is_some());
    let backup: Plan = decode(
        &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(backup.originals[0].raw, raw);
}

#[test]
fn future_format_and_uuid_paths_are_rejected_before_mutation() {
    let (_dir, registry, root, mut target) = fixture();
    target.format_version = FENCE_VERSION;
    write(
        &registry,
        root,
        MANIFEST,
        &serde_json::to_vec(&target).unwrap(),
    )
    .unwrap();
    let plane = RootControlPlane::new(registry);
    assert!(matches!(
        plane.manifest_state(root).unwrap(),
        ManifestState::Incompatible { .. }
    ));
    assert!(plane.write_record(root, &song()).is_err());
    assert!(plane.write_manifest(root, &target).is_err());
    assert!(plane
        .read_record(root, RecordKind::Song, "../../outside")
        .is_err());
    assert!(plane
        .delete_record(root, RecordKind::Song, "../../outside")
        .is_err());
}

#[cfg(unix)]
#[test]
fn record_symlink_escape_fails_before_a_backup_or_fence() {
    let (dir, registry, root, target) = fixture();
    let foreign = tempfile::tempdir().unwrap();
    let record = song();
    let relative = record_relative(&record).unwrap();
    let target_file = dir.path().join(&relative);
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    let outside = foreign.path().join("record.json");
    std::fs::write(&outside, serde_json::to_vec(&record).unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &target_file).unwrap();
    assert!(apply(
        &registry,
        root,
        &[PortableRecordEnvelope::new(record, None).unwrap()],
        &target
    )
    .is_err());
    assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
        .unwrap()
        .is_none());
    assert_eq!(
        RootControlPlane::new(registry)
            .read_manifest(root)
            .unwrap()
            .unwrap()
            .format_version,
        1
    );
}
#[test]
fn actual_mid_write_filesystem_failure_recovers_after_the_blocker_is_removed() {
    let (_dir, registry, root, target) = fixture();
    let mut records = vec![song(), song()];
    records.sort_by_cached_key(|record| record_relative(record).unwrap());
    for record in &records {
        seed(&registry, root, record);
    }
    let updates: Vec<_> = records
        .iter()
        .map(|record| PortableRecordEnvelope::new(record.clone(), None).unwrap())
        .collect();
    let blocked = path(&registry, root, &record_relative(&records[1]).unwrap()).unwrap();
    let result = apply_with_hook(&registry, root, &updates, &target, &mut |phase, index| {
        if phase == "record" && index == 1 {
            std::fs::remove_file(&blocked).unwrap();
            std::fs::create_dir(&blocked).unwrap();
        }
        Ok(())
    });
    assert!(
        result.is_err(),
        "rename onto an occupied directory must fail"
    );
    assert_eq!(
        decode::<LibraryManifest>(
            &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .format_version,
        FENCE_VERSION
    );
    std::fs::remove_dir(&blocked).unwrap();
    // Retry deliberately passes no updates: only the durable original plan
    // may decide what is replayed after a partial write.
    apply(&registry, root, &[], &target).unwrap();
    let plane = RootControlPlane::new(registry);
    assert_eq!(plane.read_manifest(root).unwrap(), Some(target));
    for record in &records {
        assert_eq!(
            plane
                .read_record(root, record.kind(), &record.object_uuid().to_string())
                .unwrap(),
            Some(record.clone())
        );
    }
}

#[test]
fn oversized_serialized_records_are_rejected_before_a_plan_is_published() {
    let (_dir, registry, root, target) = fixture();
    let mut record = song();
    if let PortableRecord::Song(value) = &mut record {
        value.title = Some("x".repeat(MAX_RECORD_BYTES));
    }
    let envelope = PortableRecordEnvelope::new(record, None).unwrap();
    assert!(apply(&registry, root, &[envelope], &target).is_err());
    assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
        .unwrap()
        .is_none());
    assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
    assert_eq!(
        RootControlPlane::new(registry)
            .read_manifest(root)
            .unwrap()
            .unwrap()
            .format_version,
        1
    );
}
