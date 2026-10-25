use super::*;
use crate::application::ports::ControlPlanePort;
use crate::domain::library::{PortableRecord, RecordKind};

fn durable_songs(fixture: &ScanFixture) -> Vec<crate::domain::library::SongRecord> {
    fixture
        .control
        .list_records(fixture.root, RecordKind::Song)
        .expect("read authoritative songs")
        .into_iter()
        .map(
            |raw| match serde_json::from_str::<PortableRecord>(&raw).expect("valid song") {
                PortableRecord::Song(song) => song,
                _ => panic!("song records only"),
            },
        )
        .collect()
}

#[test]
fn library_durability_scan_saves_identity_before_projection_and_reuses_it_on_retry() {
    let fixture = ScanFixture::new();
    fixture.write_file("saved.flac", b"audio");
    fixture.set_audio("saved.flac", "Saved", 1_000);
    fixture.database.set_fail_commit(true);

    assert!(start_scan(&fixture).run(fixture.root).is_err());
    assert!(
        fixture.all_songs().is_empty(),
        "failed projection stays invisible"
    );
    let saved = durable_songs(&fixture);
    assert_eq!(
        saved.len(),
        1,
        "the library owns the song before SQLite commits"
    );
    assert_ne!(saved[0].added_at, 0);

    fixture.database.set_fail_commit(false);
    start_scan(&fixture)
        .run(fixture.root)
        .expect("projection retry");
    let song = fixture.all_songs().pop().expect("one indexed song");
    assert_eq!(song.id().as_uuid(), saved[0].song_uuid);
    assert_eq!(song.added_at(), saved[0].added_at);
    assert_eq!(
        durable_songs(&fixture).len(),
        1,
        "retry never allocates another UUID"
    );
}

#[test]
fn library_durability_scan_refresh_preserves_saved_after_image_on_projection_failure() {
    let fixture = ScanFixture::new();
    fixture.write_file("saved.flac", b"audio");
    fixture.set_audio("saved.flac", "Original", 1_000);
    start_scan(&fixture).run(fixture.root).unwrap();
    let original = fixture.all_songs()[0].clone();
    fixture.write_file("saved.flac", b"changed synthetic audio");
    fixture.set_audio("saved.flac", "Changed", 2_000);
    fixture.database.set_fail_commit(true);
    assert!(start_scan(&fixture).run(fixture.root).is_err());
    let saved = durable_songs(&fixture);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].song_uuid, original.id().as_uuid());
    assert_eq!(saved[0].added_at, original.added_at());
    assert_eq!(saved[0].title.as_deref(), Some("Changed"));
    assert_eq!(
        saved[0].content_hash,
        blake3::hash(b"changed synthetic audio")
            .to_hex()
            .to_string()
    );
    assert_eq!(fixture.all_songs()[0].title(), original.title());
    fixture.database.set_fail_commit(false);
    start_scan(&fixture).run(fixture.root).unwrap();
    let indexed = fixture.all_songs()[0].clone();
    assert_eq!(indexed.id(), original.id());
    assert_eq!(indexed.added_at(), original.added_at());
    assert_eq!(indexed.title(), Some("Changed"));
    assert_eq!(durable_songs(&fixture), saved);
    let source = fixture
        .fs
        .root_path(fixture.root)
        .unwrap()
        .join("media/saved.flac");
    assert_eq!(
        std::fs::read(source).unwrap(),
        b"changed synthetic audio",
        "Echo reads externally changed media and never writes source bytes"
    );
}

#[test]
fn library_durability_scan_refuses_unwritable_library_before_indexing() {
    let fixture = ScanFixture::new();
    fixture.write_file("readonly.flac", b"audio");
    fixture.set_audio("readonly.flac", "Read only", 1_000);
    fixture.control.set_usable(false);

    assert!(start_scan(&fixture).run(fixture.root).is_err());
    assert!(fixture.all_songs().is_empty());
    assert!(durable_songs(&fixture).is_empty());
}

#[test]
fn library_durability_scan_preserves_record_when_media_disappears() {
    let fixture = ScanFixture::new();
    fixture.write_file("saved.flac", b"audio");
    fixture.set_audio("saved.flac", "Saved", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");
    let saved = durable_songs(&fixture);
    fixture.remove_file("saved.flac");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");
    assert_eq!(
        durable_songs(&fixture),
        saved,
        "missing is only local availability"
    );
    assert_eq!(
        fixture.all_songs()[0].availability(),
        SongAvailability::Missing
    );
}
