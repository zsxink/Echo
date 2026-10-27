//! Portable record IO contracts: typed tombstone coexistence, the future
//! format fence, and path-escape refusal.
use super::*;
use crate::application::ports::{ControlPlanePort, ManifestState};
use crate::domain::ids::Revision;
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryId, LibraryManifest, LibraryRelativePath,
    PortableRecordDocument, PortableRecordEnvelope, SongRecord, TombstoneRecord,
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
    let mut target =
        LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "upgrade-test");
    target.format_version = 2;
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

#[test]
fn typed_tombstones_with_the_same_uuid_coexist_and_delete_independently() {
    let (_dir, registry, root, target) = fixture();
    write(
        &registry,
        root,
        MANIFEST,
        &serde_json::to_vec(&target).unwrap(),
    )
    .unwrap();
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
fn future_format_and_uuid_paths_are_rejected_before_mutation() {
    let (_dir, registry, root, mut target) = fixture();
    target.format_version = u64::MAX;
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
fn record_symlink_escape_is_refused_before_any_write() {
    let (dir, registry, root, _target) = fixture();
    let foreign = tempfile::tempdir().unwrap();
    let record = song();
    let relative = record_relative(&record).unwrap();
    let target_file = dir.path().join(&relative);
    std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
    let outside = foreign.path().join("record.json");
    std::fs::write(&outside, serde_json::to_vec(&record).unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &target_file).unwrap();
    let plane = RootControlPlane::new(registry);
    assert!(plane.write_record(root, &record).is_err());
    assert!(plane
        .read_record(root, RecordKind::Song, &record.object_uuid().to_string())
        .is_err());
    assert_eq!(
        plane.read_manifest(root).unwrap().unwrap().format_version,
        1
    );
}
