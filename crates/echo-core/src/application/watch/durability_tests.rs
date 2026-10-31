//! Rename events cannot move the local projection ahead of the library.
use super::*;
use crate::application::ports::ControlPlanePort;
use crate::application::testing::ScanFixture;
use crate::domain::library::durability::DurableResource;
use crate::domain::library::{PortableRecord, RecordKind};

fn seeded() -> (ScanFixture, WatchCoordinator, Song) {
    let fixture = ScanFixture::new();
    fixture.write_file("old.mp3", b"synthetic audio");
    fixture.set_audio("old.mp3", "Song", 1_000);
    let watcher = WatchCoordinator::new(
        fixture.deps.clone(),
        Arc::new(fixture.supervisor.clone()),
        WatchCallbacks::noop(),
    );
    watcher
        .handle_event(FileEvent {
            root: fixture.root,
            path: fixture.path("old.mp3"),
            kind: FileEventKind::Created,
        })
        .unwrap();
    let song = fixture.all_songs()[0].clone();
    fixture.write_file("new.mp3", b"synthetic audio");
    fixture.remove_file("old.mp3");
    (fixture, watcher, song)
}

fn rename(fixture: &ScanFixture) -> FileEvent {
    FileEvent {
        root: fixture.root,
        path: fixture.path("new.mp3"),
        kind: FileEventKind::Renamed {
            from: fixture.path("old.mp3"),
        },
    }
}

/// Pin the source file's mtime to a whole second, which every supported
/// filesystem can represent exactly (100 ns, µs and ns granularities alike).
fn pin_source_time(fixture: &ScanFixture, path: &str, seconds: u64) {
    let absolute = fixture
        .fs
        .root_path(fixture.root)
        .expect("registered root")
        .join(fixture.path(path).normalized());
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(absolute)
        .expect("open source");
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
    )
    .expect("pin source mtime");
}

#[test]
fn library_durability_watcher_reverifies_external_edit_that_kept_size_and_mtime() {
    let fixture = ScanFixture::new();
    let original = b"external-tag";
    let edited = b"external-TAG"; // same length, different bytes
    assert_eq!(original.len(), edited.len());
    fixture.write_file("edited.mp3", original);
    fixture.set_audio("edited.mp3", "Song", 1_000);
    pin_source_time(&fixture, "edited.mp3", 1_600_000_000);
    let watcher = WatchCoordinator::new(
        fixture.deps.clone(),
        Arc::new(fixture.supervisor.clone()),
        WatchCallbacks::noop(),
    );
    watcher
        .handle_event(FileEvent {
            root: fixture.root,
            path: fixture.path("edited.mp3"),
            kind: FileEventKind::Created,
        })
        .unwrap();
    let seeded = fixture.all_songs()[0].clone();
    let stamp = seeded.file_mtime_ns();
    assert_eq!(stamp, Some(1_600_000_000_000_000_000));

    // An external tool rewrites the tags in place: same name, same length,
    // same (pinned) modification time. Only the bytes differ.
    fixture.write_file("edited.mp3", edited);
    pin_source_time(&fixture, "edited.mp3", 1_600_000_000);
    let path = fixture.path("edited.mp3");
    let meta = fixture.deps.fs.file_meta(fixture.root, &path).unwrap();
    assert_eq!(meta.size, u64::try_from(original.len()).unwrap());
    assert_eq!(Some(meta.modified_ns), stamp, "size/mtime cannot reveal it");

    let outcome = watcher
        .handle_event(FileEvent {
            root: fixture.root,
            path: path.clone(),
            kind: FileEventKind::Modified,
        })
        .unwrap();
    assert!(
        matches!(outcome, EventOutcome::Applied { song, created: false } if song == seeded.id()),
        "{outcome:?}"
    );
    // The watcher re-reads the bytes instead of trusting the change heuristic,
    // commits the externally produced hash under the same song identity, and
    // never writes the source file itself.
    let reflected = fixture.all_songs()[0].clone();
    assert_eq!(reflected.id(), seeded.id());
    assert_eq!(reflected.added_at(), seeded.added_at());
    assert_eq!(
        reflected.blake3_hash(),
        Some(blake3::hash(edited).to_hex().to_string().as_str())
    );
    let base = fixture.fs.root_path(fixture.root).unwrap();
    assert_eq!(
        std::fs::read(base.join(path.normalized())).unwrap(),
        edited,
        "Echo reads externally edited media and never writes source bytes"
    );
    let commits = fixture
        .control
        .durability()
        .unwrap()
        .recover(fixture.root)
        .unwrap()
        .commits;
    assert_eq!(commits.len(), 2, "the edit is one more directory commit");
    assert_eq!(
        commits[1].batch.resources,
        vec![DurableResource {
            relative_path: path.normalized().to_owned(),
            content_hash: blake3::hash(edited).to_hex().to_string(),
            byte_length: u64::try_from(edited.len()).unwrap(),
        }]
    );
}

#[test]
fn library_durability_watcher_rename_directory_failure_keeps_sql_path() {
    let (fixture, watcher, song) = seeded();
    fixture.control.set_usable(false);
    assert!(watcher.handle_event(rename(&fixture)).is_err());
    let indexed = fixture.deps.songs.by_id(song.id()).unwrap().unwrap();
    assert_eq!(indexed.path(), song.path());
    assert_eq!(indexed.added_at(), song.added_at());
}

#[test]
fn library_durability_watcher_rename_saves_identity_before_failed_projection() {
    let (fixture, watcher, song) = seeded();
    fixture.database.set_fail_commit(true);
    let error = watcher.handle_event(rename(&fixture)).unwrap_err();
    assert!(
        matches!(error, Error::LibrarySavedPending { .. }),
        "{error:?}"
    );
    assert_eq!(
        fixture.deps.songs.by_id(song.id()).unwrap().unwrap().path(),
        song.path()
    );
    let Some(PortableRecord::Song(saved)) = fixture
        .control
        .read_record(fixture.root, RecordKind::Song, &song.id().to_string())
        .unwrap()
    else {
        panic!("saved renamed song expected");
    };
    assert_eq!(saved.media_path.as_str(), "media/new.mp3");
    assert_eq!(saved.song_uuid, song.id().as_uuid());
    assert_eq!(saved.added_at, song.added_at());
    fixture.database.set_fail_commit(false);
    watcher.handle_event(rename(&fixture)).unwrap();
    let indexed = fixture.deps.songs.by_id(song.id()).unwrap().unwrap();
    assert_eq!(indexed.path(), &fixture.path("new.mp3"));
    assert_eq!(indexed.added_at(), song.added_at());
}
