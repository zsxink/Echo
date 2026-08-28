use std::sync::Arc;
use std::time::Duration;

use rusqlite::params;

use super::*;
use crate::application::ports::{
    LibraryRepository, OperationResourceKind, PlaylistRepository, SongRepository, UnitOfWork,
};
use crate::domain::catalog::{SongSortField, SortDirection};
use crate::domain::ids::Revision;
use crate::domain::state::OperationState;

fn database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open database");
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .expect("insert active root");
    (directory, database, root)
}

fn song(root: LibraryRootId, path: &str, title: &str, artist: &str) -> Song {
    let mut song = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new(path).expect("valid path"),
        Revision::INITIAL,
    );
    song.apply_metadata(
        Some(title.to_owned()),
        Some(artist.to_owned()),
        Some("专辑".to_owned()),
        Some(Duration::from_secs(180)),
    );
    song
}

#[test]
fn initial_migration_has_required_tables_indexes_and_no_sync_tables() {
    let (_directory, database, _) = database();
    let objects = database.schema_snapshot().expect("schema snapshot");
    let names: Vec<_> = objects.iter().map(|(name, _)| name.as_str()).collect();
    for required in [
        "schema_migrations",
        "library_roots",
        "songs",
        "song_lyrics",
        "song_overrides",
        "cover_assets",
        "playlists",
        "playlist_songs",
        "operation_journal",
        "operation_items",
        "scan_runs",
        "scan_issues",
        "recorded_play_sessions",
        "song_search",
        "operation_items_active_target_claim",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
    assert!(!names
        .iter()
        .any(|name| name.contains("sync") || name.contains("tombstone")));
    assert!(database.quick_check().is_ok());
}

#[test]
fn schema_constraints_cover_active_root_paths_playlist_and_target_claim() {
    let (directory, database, root) = database();
    let duplicate_root = LibraryRoot::new(
        LibraryRootId::new(),
        directory.path().join("other"),
        true,
        true,
    );
    LibraryRepository::upsert(&database, &duplicate_root)
        .expect("actor atomically swaps active root");
    assert_eq!(
        database.active_root().expect("active").expect("root").id(),
        duplicate_root.id()
    );
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .expect("restore root");
    let first = song(root, "歌手/相同.flac", "相同", "歌手");
    SongRepository::upsert(&database, &first).expect("insert song");
    let duplicate_path = song(root, "歌手/相同.flac", "不同", "歌手");
    assert!(SongRepository::upsert(&database, &duplicate_path).is_err());
    let playlist = PlaylistId::new();
    database.create(playlist, root, "最爱").expect("playlist");
    assert!(database
        .create(PlaylistId::new(), root, "  最爱  ")
        .is_err());
    database
        .add_member(playlist, first.id(), u64::MAX)
        .expect("member");
    database
        .add_member(playlist, first.id(), u64::MAX)
        .expect("idempotent member");
    assert_eq!(database.members(playlist).expect("members").len(), 1);
    let operation = OperationId::new();
    database
        .create_operation(operation, root, "import", Some(first.id()))
        .expect("operation");
    database
        .upsert_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::Planned,
                song: Some(first.id()),
                target_path: RelativeMediaPath::new("新/歌.flac").expect("path"),
                expected_hash: "a".repeat(64),
                claim_key: "audio".to_owned(),
            },
        )
        .expect("claim");
    let second_operation = OperationId::new();
    database
        .create_operation(second_operation, root, "import", None)
        .expect("operation");
    assert!(database
        .upsert_item(
            second_operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::Planned,
                song: None,
                target_path: RelativeMediaPath::new("新/歌.flac").expect("path"),
                expected_hash: "b".repeat(64),
                claim_key: "audio".to_owned()
            }
        )
        .is_err());
}

#[test]
fn unit_of_work_rolls_back_and_real_repositories_round_trip() {
    let (_directory, database, root) = database();
    let rolled_back = song(root, "a.flac", "A", "甲");
    let result: Result<(), Error> = database.with_tx(move |tx| {
        tx.upsert_song(&rolled_back)?;
        Err(Error::Cancelled)
    });
    assert!(result.is_err());
    assert!(database
        .by_path(root, &RelativeMediaPath::new("a.flac").expect("path"))
        .expect("query")
        .is_none());
    let committed = song(root, "b.flac", "B", "乙");
    database
        .with_tx({
            let committed = committed.clone();
            move |tx| tx.upsert_song(&committed)
        })
        .expect("commit");
    assert_eq!(
        SongRepository::by_id(&database, committed.id())
            .expect("song")
            .expect("present")
            .title(),
        Some("B")
    );
}

#[test]
fn fts_and_short_like_search_are_unicode_safe_and_complete() {
    let (_directory, database, root) = database();
    for value in [
        ("中文.flac", "晴天", "周杰伦"),
        ("日本.flac", "夜に駆ける", "YOASOBI"),
        ("latin.flac", "Café", "ARTIST"),
    ] {
        SongRepository::upsert(&database, &song(root, value.0, value.1, value.2)).expect("song");
    }
    assert_eq!(
        database
            .search_active_songs("周杰伦", SongSort::default(), None, 20)
            .expect("CJK FTS")
            .items
            .len(),
        1
    );
    assert_eq!(
        database
            .search_active_songs("駆け", SongSort::default(), None, 20)
            .expect("Japanese FTS")
            .items
            .len(),
        1
    );
    assert_eq!(
        database
            .search_active_songs("café", SongSort::default(), None, 20)
            .expect("case folded FTS")
            .items
            .len(),
        1
    );
    assert_eq!(
        database
            .search_active_songs("晴", SongSort::default(), None, 20)
            .expect("one character LIKE")
            .items
            .len(),
        1
    );
    assert_eq!(
        database
            .search_active_songs("%_\" OR *", SongSort::default(), None, 20)
            .expect("escaped query")
            .items
            .len(),
        0
    );
}

#[test]
fn keyset_pages_are_deterministic_and_reject_stale_cursors() {
    let (_directory, database, root) = database();
    for index in 0..6 {
        SongRepository::upsert(
            &database,
            &song(root, &format!("{index}.flac"), "same", "artist"),
        )
        .expect("song");
    }
    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    let first = database
        .query_active_songs("", sort, None, 2)
        .expect("first page");
    let cursor = first.next_cursor.clone().expect("next cursor");
    let second = database
        .query_active_songs("", sort, Some(&cursor), 2)
        .expect("second page");
    assert!(first
        .items
        .iter()
        .all(|left| second.items.iter().all(|right| left.id() != right.id())));
    SongRepository::upsert(&database, &song(root, "later.flac", "later", "artist"))
        .expect("write changes revision");
    assert!(database
        .query_active_songs("", sort, Some(&cursor), 2)
        .is_err());
    assert_eq!(database.recent_songs().expect("recent").len(), 7);
}

#[test]
fn playback_sessions_are_idempotent_and_writer_allows_concurrent_reads() {
    let (_directory, database, root) = database();
    let song = song(root, "play.flac", "play", "artist");
    SongRepository::upsert(&database, &song).expect("song");
    let session = PlaybackSessionId::new();
    assert!(database
        .record_playback(session, song.id())
        .expect("first play"));
    assert!(!database
        .record_playback(session, song.id())
        .expect("duplicate play"));
    assert!(database
        .record_playback(PlaybackSessionId::new(), song.id())
        .expect("new session"));
    assert_eq!(
        SongRepository::by_id(&database, song.id())
            .expect("song")
            .expect("present")
            .play_count()
            .as_u64(),
        2
    );
    let database = Arc::new(database);
    let mut handles = Vec::new();
    for _ in 0..4 {
        let database = Arc::clone(&database);
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                database
                    .query_active_songs("pl", SongSort::default(), None, 10)
                    .expect("read while writer idle");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("reader thread");
    }
}

#[test]
fn migration_checksum_and_backup_are_reopenable() {
    let (directory, database, _) = database();
    let backup = directory.path().join("backup.db");
    database.backup_to(&backup).expect("backup");
    drop(database);
    let backup_database = SqliteDatabase::open(&backup).expect("reopen backup");
    assert!(backup_database.quick_check().is_ok());
    let mut direct = open_writer(&directory.path().join("checksum.db")).expect("connection");
    apply_migrations(&mut direct).expect("migration");
    direct
        .execute(
            "UPDATE schema_migrations SET checksum = 'changed' WHERE version = 1",
            params![],
        )
        .expect("tamper");
    assert!(apply_migrations(&mut direct).is_err());
    let failed = [(
        2,
        "CREATE TABLE rollback_marker (id INTEGER); CREATE TABLE rollback_marker (id INTEGER);",
    )];
    assert!(apply_migration_set(&mut direct, &failed).is_err());
    assert!(direct.prepare("SELECT * FROM rollback_marker").is_err());
}
