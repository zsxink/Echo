use super::*;
use crate::application::ports::{ControlPlanePort, LibraryHistoryPort, LibraryProjectionPort};
use crate::domain::library::{
    durability::{LibraryProjectionStatus, LibraryRecoveryRecords, LibraryReplay},
    PortableRecord, RecordKind,
};
use std::sync::Arc;

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
fn library_durability_small_scan_commits_one_record_and_resource_per_song() {
    use crate::application::ports::ControlPlanePort as _;

    let fixture = ScanFixture::new();
    for (path, bytes, title) in [
        ("small/a.flac", b"audio-a".as_slice(), "A"),
        ("small/b.flac", b"audio-b".as_slice(), "B"),
        ("small/c.flac", b"audio-c".as_slice(), "C"),
    ] {
        fixture.write_file(path, bytes);
        fixture.set_audio(path, title, 1_000);
    }

    start_scan(&fixture).run(fixture.root).expect("small scan");
    let replay = fixture
        .control
        .durability()
        .unwrap()
        .recover(fixture.root)
        .unwrap();
    assert_eq!(fixture.all_songs().len(), 3);
    assert_eq!(replay.commits.len(), 3, "one authority commit per new song");
    assert!(replay
        .commits
        .iter()
        .all(|commit| { commit.batch.records.len() == 1 && commit.batch.resources.len() == 1 }));

    start_scan(&fixture)
        .run(fixture.root)
        .expect("unchanged rescan");
    let repeated = fixture
        .control
        .durability()
        .unwrap()
        .recover(fixture.root)
        .unwrap();
    assert_eq!(
        repeated.commits.len(),
        3,
        "unchanged scan adds no IO commits"
    );
    assert_eq!(
        fixture.all_songs().len(),
        3,
        "identity allocation is stable"
    );
}

#[derive(Clone)]
struct RefusingProjection;

impl LibraryProjectionPort for RefusingProjection {
    fn project_library_commits(
        &self,
        _root: crate::domain::ids::LibraryRootId,
        _replay: &LibraryReplay,
    ) -> Result<(), crate::error::Error> {
        Err(crate::error::Error::unavailable(
            "test projection",
            "injected ordered replay failure",
        ))
    }

    fn projection_status(
        &self,
        _root: crate::domain::ids::LibraryRootId,
    ) -> Result<LibraryProjectionStatus, crate::error::Error> {
        Ok(LibraryProjectionStatus::default())
    }

    fn read_recovery_records(
        &self,
        _root: crate::domain::ids::LibraryRootId,
    ) -> Result<LibraryRecoveryRecords, crate::error::Error> {
        Ok(LibraryRecoveryRecords::default())
    }
}

impl LibraryHistoryPort for RefusingProjection {
    fn capture_library_history(
        &self,
        _root: crate::domain::ids::LibraryRootId,
    ) -> Result<Vec<crate::domain::library::PortableRecordEnvelope>, crate::error::Error> {
        Ok(Vec::new())
    }
}

#[test]
fn library_durability_song_commit_runs_ordered_replay_before_site_projection() {
    use crate::domain::entities::Song;
    use crate::domain::ids::{OperationId, RelativeMediaPath, Revision, SongId};
    use crate::domain::library::durability::{ChangeOrigin, DurableResource};
    use std::sync::atomic::{AtomicBool, Ordering};

    let fixture = ScanFixture::new();
    let mut deps = (*fixture.deps).clone();
    deps.history = Some(Arc::new(RefusingProjection));
    let library = super::durable::library_id(&deps, fixture.root).unwrap();
    let first_operation = OperationId::new();
    let first_song =
        crate::application::testkit::library_durability::Issue72Fixture::with_database_evidence()
            .database
            .into_iter()
            .find(|record| matches!(record, PortableRecord::Song(_)))
            .unwrap();
    deps.control
        .durability()
        .unwrap()
        .commit(
            fixture.root,
            &crate::domain::library::durability::LibraryBatch::new(
                first_operation.as_uuid(),
                library,
                ChangeOrigin::Import,
                vec![
                    crate::domain::library::PortableRecordEnvelope::new(first_song, None).unwrap(),
                ],
            ),
        )
        .unwrap();
    let mut song = Song::new(
        SongId::from_uuid(uuid::Uuid::new_v4()),
        fixture.root,
        RelativeMediaPath::new("media/saved.flac").unwrap(),
        Revision::INITIAL,
    );
    let content_hash = blake3::hash(b"synthetic audio").to_hex().to_string();
    song.apply_scan_facts(
        content_hash.clone(),
        15,
        0,
        crate::domain::media::AudioFormat::Flac,
    );
    let operation = OperationId::new();
    let local_projection_ran = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&local_projection_ran);

    let error = super::durable::commit_song(
        &deps,
        &song,
        operation,
        ChangeOrigin::Import,
        vec![DurableResource {
            relative_path: song.path().display().to_owned(),
            content_hash,
            byte_length: 15,
        }],
        move || {
            observed.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect_err("ordered projection refuses before site projection");

    assert!(matches!(
        error,
        crate::error::Error::LibrarySavedPending { operation, .. }
            if operation == first_operation
    ));
    assert!(!local_projection_ran.load(Ordering::SeqCst));
    assert_eq!(durable_songs(&fixture).len(), 2);
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
