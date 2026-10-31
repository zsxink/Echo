use super::*;
use crate::application::ports::ControlPlanePort;
use crate::domain::ids::{LibraryRootId, Revision};
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryId, LibraryRelativePath, SongRecord,
};

fn setup() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    RootControlPlane,
) {
    let dir = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, dir.path());
    let plane = RootControlPlane::new(registry.clone());
    (dir, registry, root, plane)
}

#[test]
fn library_durability_writer_permit_releases_lock_with_a_duplicated_handle_alive() {
    let (_dir, registry, root, plane) = setup();
    let guard = plane.prepare_write(root, true).unwrap();
    // A concurrent process spawn can temporarily inherit an open file
    // description before close-on-exec. Its lifetime is not a write permit.
    let inherited = guard.process.try_clone().unwrap();
    drop(guard);
    let next = RootControlPlane::new(registry);
    let permit = next.prepare_write(root, true).unwrap();
    drop(permit);
    drop(inherited);
}

#[test]
// The competing adapter must attempt acquisition before the guard is
// consumed by the deliberately failing preparation.
#[allow(clippy::significant_drop_tightening)]
fn library_durability_failed_write_preparation_unlocks_with_a_duplicated_handle_alive() {
    let (directory, registry, root, plane) = setup();
    let invalid = directory.path().join("echo").join("manifest.json");
    std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
    std::fs::write(&invalid, "not valid JSON").unwrap();

    let guard = plane.acquire_write(root).unwrap();
    let inherited = guard.process.try_clone().unwrap();
    let competing = RootControlPlane::new(registry);
    assert!(competing.acquire_write(root).is_err());
    assert!(plane.prepare_locked_write(root, false, guard).is_err());

    // Recovery and manifest errors release the acquired permit even
    // while another handle keeps its file description alive.
    std::fs::remove_file(invalid).unwrap();
    drop(competing.prepare_write(root, false).unwrap());
    // The adapter's own thread gate must also be released on error.
    drop(plane.prepare_write(root, false).unwrap());
    drop(inherited);
}

mod write_boundaries;

fn sample_record(root_uuid: uuid::Uuid) -> PortableRecord {
    PortableRecord::Song(SongRecord {
        song_uuid: root_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
        ),
        hlc: HybridLogicalClock::new(1_700_000_000, 0),
        media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
        content_hash: "abc123".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("晴天".to_owned()),
        artist: Some("周杰伦".to_owned()),
        album: None,
    })
}

#[test]
fn manifest_round_trips_atomically() {
    let (_dir, _registry, root, plane) = setup();
    let manifest = LibraryManifest::new(
        LibraryId::from_uuid(uuid::Uuid::new_v4()),
        "0.1.0".to_owned(),
    );
    plane
        .write_manifest(root, &manifest)
        .expect("write manifest");
    let read = plane
        .read_manifest(root)
        .expect("read manifest")
        .expect("present");
    assert_eq!(read, manifest);
    assert!(read.is_compatible());
}

#[test]
fn manifest_absent_is_none() {
    let (_dir, _registry, root, plane) = setup();
    assert!(plane.read_manifest(root).expect("read").is_none());
}

#[test]
fn record_round_trips_through_shard_path() {
    let (dir, _registry, root, plane) = setup();
    let song_uuid = uuid::Uuid::new_v4();
    let record = sample_record(song_uuid);
    plane.write_record(root, &record).expect("write record");
    let read = plane
        .read_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("read record")
        .expect("present");
    assert_eq!(read, record);

    // The file lives in the prefix-sharded path.
    let expected_prefix = &song_uuid.to_string()[..2];
    let echo = dir.path().join("echo");
    let record_path = echo
        .join("records")
        .join("songs")
        .join(expected_prefix)
        .join(format!("{song_uuid}.json"));
    assert!(record_path.is_file(), "record at sharded path");
}

#[test]
fn record_absent_is_none() {
    let (_dir, _registry, root, plane) = setup();
    assert!(plane
        .read_record(root, RecordKind::Song, &uuid::Uuid::new_v4().to_string())
        .expect("read")
        .is_none());
}

#[test]
fn delete_record_is_idempotent() {
    let (_dir, _registry, root, plane) = setup();
    let song_uuid = uuid::Uuid::new_v4();
    plane
        .write_record(root, &sample_record(song_uuid))
        .expect("write");
    plane
        .delete_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("delete");
    assert!(plane
        .read_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("read")
        .is_none());
    // Deleting a missing record is a no-op.
    plane
        .delete_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("delete again");
}

#[test]
fn tmp_files_are_ignored_as_records() {
    let (dir, _registry, root, plane) = setup();
    let song_uuid = uuid::Uuid::new_v4();
    // Write the real record, then place a same-dir temp file with a partial
    // JSON body (simulating an interrupted atomic write).
    plane
        .write_record(root, &sample_record(song_uuid))
        .expect("write real record");
    let echo = dir.path().join("echo");
    let record_path = echo
        .join("records")
        .join("songs")
        .join(&song_uuid.to_string()[..2])
        .join(format!("{song_uuid}.json"));
    let temp = record_path.with_file_name(format!("{song_uuid}.json.tmp-deadbeef"));
    std::fs::write(&temp, "{\"partial\": true, \"unfinished").expect("write temp");
    // The reader still sees the real record; the temp is invisible.
    assert!(plane
        .read_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("read")
        .is_some());
    // The temp never appears in listings.
    let listed = plane
        .list_records(root, RecordKind::Song)
        .expect("list records");
    assert_eq!(listed.len(), 1, "only the real record is listed");
    assert!(!listed[0].contains("unfinished"), "no partial body listed");

    // A malformed *published* record (no temp) is ignored, not surfaced.
    std::fs::write(&record_path, "{\"broken\": ").expect("corrupt record");
    assert!(plane
        .read_record(root, RecordKind::Song, &song_uuid.to_string())
        .expect("read")
        .is_none());
    let listed = plane.list_records(root, RecordKind::Song).expect("list");
    assert_eq!(
        listed.len(),
        0,
        "malformed records are not surfaced as valid"
    );
}

#[test]
fn unknown_format_manifest_is_absent() {
    let (dir, _registry, root, plane) = setup();
    let echo = dir.path().join("echo");
    std::fs::create_dir_all(&echo).expect("echo dir");
    // A future-format manifest must be treated as incompatible/absent.
    std::fs::write(
        echo.join("manifest.json"),
        format!(
            "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"0.1.0\"}}",
            uuid::Uuid::new_v4()
        ),
    )
    .expect("write future manifest");
    assert!(plane.read_manifest(root).expect("read").is_none());
}

#[test]
fn control_plane_usable_detects_writable_and_readable() {
    let (_dir, _registry, root, plane) = setup();
    assert!(plane.control_plane_usable(root).expect("usable check"));
    // A usable plane writes manifest fine.
    plane
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0"),
        )
        .expect("write manifest");
}

#[test]
fn manifest_state_separates_absent_compatible_and_future() {
    let (dir, _registry, root, plane) = setup();
    // 1. Absent: nothing written yet.
    assert_eq!(
        plane.manifest_state(root).expect("state"),
        ManifestState::Absent
    );
    assert!(!plane.records_present(root).expect("records"));

    // 2. Compatible: this build's own manifest.
    let manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0");
    plane.write_manifest(root, &manifest).expect("write");
    assert_eq!(
        plane.manifest_state(root).expect("state"),
        ManifestState::Compatible(manifest)
    );

    // 3. Future format: present but untouchable — the self-heal path must
    //    never see this as "absent".
    let echo = dir.path().join("echo");
    std::fs::write(
        echo.join("manifest.json"),
        format!(
            "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"9.9.9\"}}",
            uuid::Uuid::new_v4()
        ),
    )
    .expect("write future manifest");
    assert_eq!(
        plane.manifest_state(root).expect("state"),
        ManifestState::Incompatible { format_version: 99 }
    );

    // 4. Unparseable: also a refusal, not a self-heal trigger.
    std::fs::write(echo.join("manifest.json"), "not json at all").expect("corrupt manifest");
    assert_eq!(
        plane.manifest_state(root).expect("state"),
        ManifestState::Malformed
    );
}

#[test]
fn records_present_ignores_temporaries_and_counts_every_kind() {
    let (dir, _registry, root, plane) = setup();
    // A kind directory holding only a temp file is *not* "records present"
    // (an interrupted write must not turn a fresh root into a heal target).
    let echo = dir.path().join("echo");
    let songs = echo.join("records").join("songs").join("ab");
    std::fs::create_dir_all(&songs).expect("shard dir");
    std::fs::write(songs.join("x.json.tmp-1"), "{}").expect("temp");
    assert!(!plane.records_present(root).expect("records"));

    // A real record of any kind makes the directory a continuation target.
    let song_uuid = uuid::Uuid::new_v4();
    plane
        .write_record(root, &sample_record(song_uuid))
        .expect("write song record");
    assert!(plane.records_present(root).expect("records"));

    let fav_uuid = uuid::Uuid::new_v4();
    plane
        .write_record(
            root,
            &PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
                song_uuid: fav_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::default(),
                is_favorite: true,
                favorited_at: None,
            }),
        )
        .expect("write favorite record");
    assert!(plane.records_present(root).expect("records"));
}

#[test]
fn list_records_sorts_and_multiples() {
    let (_dir, _registry, root, plane) = setup();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    plane
        .write_record(root, &sample_record(a))
        .expect("write a");
    plane
        .write_record(root, &sample_record(b))
        .expect("write b");
    let listed = plane.list_records(root, RecordKind::Song).expect("list");
    assert_eq!(listed.len(), 2, "two songs listed");
    // Different kinds don't collide.
    let fav = PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
        song_uuid: a,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::default(),
        is_favorite: true,
        favorited_at: None,
    });
    plane.write_record(root, &fav).expect("write favorite");
    assert_eq!(
        plane
            .list_records(root, RecordKind::Song)
            .expect("songs")
            .len(),
        2
    );
    assert_eq!(
        plane
            .list_records(root, RecordKind::Favorite)
            .expect("favorites")
            .len(),
        1
    );
}
