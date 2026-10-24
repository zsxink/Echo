//! Rename events cannot move the local projection ahead of the library.
use super::*;
use crate::application::ports::ControlPlanePort;
use crate::application::testing::ScanFixture;
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
