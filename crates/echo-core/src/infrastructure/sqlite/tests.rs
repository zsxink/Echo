use std::sync::Arc;
use std::time::Duration;

use rusqlite::params;

use super::*;
use crate::application::catalog::CatalogQuery;
use crate::application::ports::{
    LibraryRepository, OperationResourceKind, PlaylistRepository, SongRepository, UnitOfWork,
};
use crate::domain::catalog::{SongSort, SongSortField, SortDirection};
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
                source: None,
                staging_path: None,
                target_path: RelativeMediaPath::new("新/歌.flac").expect("path"),
                expected_hash: "a".repeat(64),
                item_key: "audio".to_owned(),
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
                source: None,
                staging_path: None,
                target_path: RelativeMediaPath::new("新/歌.flac").expect("path"),
                expected_hash: "b".repeat(64),
                item_key: "audio".to_owned(),
                claim_key: "audio".to_owned()
            }
        )
        .is_err());
}

#[test]
fn unit_of_work_rolls_back_and_real_repositories_round_trip() {
    let (_directory, database, root) = database();
    let rolled_back = song(root, "a.flac", "A", "甲");
    let result: Result<(), Error> = database.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        tx.upsert_song(&rolled_back)?;
        Err(Error::Cancelled)
    }));
    assert!(result.is_err());
    assert!(database
        .by_path(root, &RelativeMediaPath::new("a.flac").expect("path"))
        .expect("query")
        .is_none());
    let committed = song(root, "b.flac", "B", "乙");
    database
        .with_tx({
            let committed = committed.clone();
            Box::new(move |tx: &mut dyn TxAccess| tx.upsert_song(&committed))
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
    let mut inserted = Vec::new();
    for index in 0..6 {
        let song = song(root, &format!("{index}.flac"), "same", "artist");
        SongRepository::upsert(&database, &song).expect("song");
        inserted.push(song);
    }

    // Every sort in both directions: keyset pagination must cover each song
    // exactly once, and a full re-pagination must reproduce the same order.
    for field in SongSortField::ALL {
        for direction in [SortDirection::Asc, SortDirection::Desc] {
            let sort = SongSort { field, direction };
            let first_pass = paginate_all(&database, sort, 2);
            let second_pass = paginate_all(&database, sort, 2);
            assert_eq!(first_pass.len(), 6, "{field:?} {direction:?}");
            let mut unique = first_pass.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), 6, "no duplicate rows across pages");
            assert_eq!(first_pass, second_pass, "pagination is deterministic");

            // In-memory domain ordering and SQL keyset ordering agree row for
            // row (the sort ladders are defined once, not twice).
            let mut domain_sorted = inserted.clone();
            domain_sorted.sort_by(|left, right| sort.compare(left, right));
            let domain_ids: Vec<_> = domain_sorted.iter().map(Song::id).collect();
            assert_eq!(first_pass, domain_ids, "domain and SQL order agree");
        }
    }

    // A rescan (re-upsert of unchanged songs) must not reorder the catalog.
    for song in &inserted {
        SongRepository::upsert(&database, song).expect("rescan upsert");
    }
    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    assert_eq!(
        paginate_all(&database, sort, 2),
        paginate_all(&database, sort, 2)
    );
    let after_rescan = paginate_all(&database, sort, 2);

    // A write after the cursor was minted invalidates it (revision guard).
    let first = database
        .query_active_songs("", sort, None, 2)
        .expect("first page");
    let cursor = first.next_cursor.expect("next cursor");
    SongRepository::upsert(&database, &song(root, "later.flac", "later", "artist"))
        .expect("write changes revision");
    assert!(database
        .query_active_songs("", sort, Some(&cursor), 2)
        .is_err());

    // …but pagination from scratch stays deterministic after the rescan plus
    // the insertion: the six rescanned songs keep their exact relative order
    // and the new song just takes its deterministic place in the ladder.
    let full_after_insert = paginate_all(&database, sort, 500);
    assert_eq!(full_after_insert.len(), 7);
    let kept: Vec<SongId> = full_after_insert
        .into_iter()
        .filter(|id| inserted.iter().any(|s| s.id() == *id))
        .collect();
    assert_eq!(
        kept, after_rescan,
        "rescan must not reorder; an insert only slots in"
    );
    assert_eq!(database.recent_songs().expect("recent").len(), 7);
}

/// Walk every page of the active root and return the song ids in order.
fn paginate_all(database: &SqliteDatabase, sort: SongSort, limit: usize) -> Vec<SongId> {
    let mut ids = Vec::new();
    let mut cursor: Option<OpaqueCursor> = None;
    loop {
        let page = database
            .query_active_songs("", sort, cursor.as_ref(), limit)
            .expect("page");
        ids.extend(page.items.iter().map(Song::id));
        if page.is_last {
            return ids;
        }
        cursor = page.next_cursor;
        assert!(cursor.is_some(), "non-last page must carry a cursor");
    }
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

    // Real concurrency: one writer mutating while several readers page the
    // catalog through the bounded read pool. Neither side may observe busy
    // errors or a torn snapshot, and readers are capped at `reader_count()`
    // connections (they block instead of growing the pool).
    let database = Arc::new(database);
    let writer = {
        let database = Arc::clone(&database);
        let song_id = song.id();
        std::thread::spawn(move || {
            for round in 0..40u64 {
                database
                    .record_playback(PlaybackSessionId::new(), song_id)
                    .expect("concurrent record");
                SongRepository::set_favorite(&*database, song_id, round % 2 == 0)
                    .expect("concurrent favorite");
            }
        })
    };
    let mut handles = Vec::new();
    for _ in 0..6 {
        let database = Arc::clone(&database);
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                let sort = SongSort::default();
                let page = database
                    .query_active_songs("pl", sort, None, 10)
                    .expect("read while the writer commits");
                assert_eq!(page.items.len(), 1);
                database.recent_songs().expect("recent while writer active");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("reader thread");
    }
    writer.join().expect("writer thread");

    // Every writer mutation committed: 2 pre-concurrency sessions + 40
    // concurrent sessions, all through the single-writer actor.
    assert_eq!(
        SongRepository::by_id(&*database, song.id())
            .expect("song")
            .expect("present")
            .play_count()
            .as_u64(),
        42
    );
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

fn claim_item(song: Option<SongId>, target: &str, key: &str) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Audio,
        state: OperationState::Planned,
        song,
        source: None,
        staging_path: None,
        target_path: RelativeMediaPath::new(target).expect("path"),
        expected_hash: "a".repeat(64),
        item_key: key.to_owned(),
        claim_key: key.to_owned(),
    }
}

#[test]
fn stale_scan_upsert_preserves_favorite_play_count_and_availability() {
    let (_directory, database, root) = database();
    let original = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &original).expect("insert");
    SongRepository::set_favorite(&database, original.id(), true).expect("favorite");
    SongRepository::increment_play_count(&database, original.id()).expect("play");

    // A scan writes back a stale snapshot: refreshed metadata, but the
    // in-memory copy was taken before the favorite/play/availability
    // mutations, so it carries favorite=false, count=0 and `missing`.
    let mut stale = original.clone();
    stale.apply_metadata(Some("A2".to_owned()), None, None, None);
    stale.mark_missing();
    SongRepository::upsert(&database, &stale).expect("stale write-back");

    let stored = SongRepository::by_id(&database, original.id())
        .expect("query")
        .expect("present");
    assert!(stored.favorite(), "favorite must survive a stale upsert");
    assert_eq!(
        stored.play_count().as_u64(),
        1,
        "play count must survive a stale upsert"
    );
    assert_eq!(
        stored.availability(),
        SongAvailability::Available,
        "availability only changes through its dedicated mutation"
    );
    // The metadata itself did update.
    assert_eq!(stored.title(), Some("A2"));
}

#[test]
fn pending_delete_hides_from_catalog_and_finalize_removes_members_atomically() {
    let (_directory, database, root) = database();
    let song = song(root, "pd.flac", "PD", "甲");
    SongRepository::upsert(&database, &song).expect("song");
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, song.id(), u64::MAX)
        .expect("member");

    // Pending delete hides the song from catalog views…
    SongRepository::set_availability(&database, song.id(), SongAvailability::PendingDelete)
        .expect("pending delete");
    assert!(database
        .query_active_songs("", SongSort::default(), None, 10)
        .expect("page")
        .items
        .is_empty());
    // …while identity and associations are kept.
    let stored = SongRepository::by_id(&database, song.id())
        .expect("query")
        .expect("present");
    assert_eq!(stored.availability(), SongAvailability::PendingDelete);
    assert_eq!(database.members(playlist).expect("members").len(), 1);

    // Delete finalization removes the membership rows inside one transaction.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.remove_member(playlist, song.id())
        }))
        .expect("finalize");
    assert!(database.members(playlist).expect("members").is_empty());
    assert!(database.delete(playlist).is_ok());
}

#[test]
fn playlist_members_reject_cross_root_and_position_conflicts() {
    let (directory, database, root) = database();
    let root_two = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root_two, directory.path().join("two"), false, true),
    )
    .expect("second (inactive) root");
    let song_a = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song_a).expect("a");
    let song_b = song(root_two, "b.flac", "B", "乙");
    SongRepository::upsert(&database, &song_b).expect("b");
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");

    // A playlist never references songs of another library root.
    assert!(
        database
            .add_member(playlist, song_b.id(), u64::MAX)
            .is_err(),
        "cross-root membership must be rejected"
    );

    // Appending takes the next free position; re-adding is idempotent…
    database
        .add_member(playlist, song_a.id(), u64::MAX)
        .expect("append a at 0");
    // …but an explicit position clash with another member is a real conflict,
    // never a silent no-op (no INSERT OR IGNORE on position).
    let other = song(root, "c.flac", "C", "丙");
    SongRepository::upsert(&database, &other).expect("c");
    assert!(
        database.add_member(playlist, other.id(), 0).is_err(),
        "position clash must surface as an error"
    );
    database
        .add_member(playlist, song_a.id(), u64::MAX)
        .expect("re-adding an existing member is idempotent");
    database
        .add_member(playlist, other.id(), u64::MAX)
        .expect("append c at 1");

    let members = database.members(playlist).expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].song(), song_a.id());
    assert_eq!(members[0].position(), 0);
    assert_eq!(members[1].song(), other.id());
    assert_eq!(members[1].position(), 1);
}

#[test]
fn target_claims_block_other_operations_until_released() {
    let (_directory, database, root) = database();
    let song = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    let first = OperationId::new();
    database
        .create_operation(first, root, "import", Some(song.id()))
        .expect("envelope");
    database
        .upsert_item(first, claim_item(Some(song.id()), "新/歌.flac", "audio"))
        .expect("claim");

    // A second operation cannot claim the same target path while the first
    // claim is active…
    let second = OperationId::new();
    database
        .create_operation(second, root, "import", None)
        .expect("envelope");
    assert!(database
        .upsert_item(second, claim_item(None, "新/歌.flac", "audio"))
        .is_err());

    // …and a terminal operation releases its claims, freeing the path.
    database.release_claims(first).expect("release");
    database
        .upsert_item(second, claim_item(None, "新/歌.flac", "audio"))
        .expect("re-claim after release");
}

#[test]
fn song_lyrics_keep_multiple_candidates_for_fallback() {
    let (_directory, database, root) = database();
    let song = song(root, "lyrics.flac", "L", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    // Write through a side connection: the lyrics repository itself arrives
    // with the scan pipeline; this test pins the storage contract only.
    let raw = rusqlite::Connection::open(database.path()).expect("side connection");
    for (source, kind) in [
        ("embedded", "timed"),
        ("sidecar", "plain"),
        ("override", "plain"),
    ] {
        raw.execute(
            "INSERT INTO song_lyrics (song_uuid, source, text_kind, raw_text, updated_at) VALUES (?1, ?2, ?3, 'raw', 1)",
            params![song.id().to_string(), source, kind],
        )
        .expect("candidate row");
    }
    // One candidate per source: a second embedded row is rejected…
    assert!(raw
        .execute(
            "INSERT INTO song_lyrics (song_uuid, source, text_kind, updated_at) VALUES (?1, 'embedded', 'timed', 2)",
            params![song.id().to_string()],
        )
        .is_err());
    // …and all three candidates coexist for priority selection with fallback.
    let count: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM song_lyrics WHERE song_uuid = ?1",
            params![song.id().to_string()],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(count, 3);
}

#[test]
fn multi_playlist_membership_commits_in_one_transaction() {
    let (_directory, database, root) = database();
    let song_a = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song_a).expect("song");
    let song_id = song_a.id();
    let playlist_one = PlaylistId::new();
    let playlist_two = PlaylistId::new();
    database.create(playlist_one, root, "其一").expect("one");
    database.create(playlist_two, root, "其二").expect("two");

    // Adding the same song to several playlists is one atomic snapshot: the
    // transaction either commits every membership or none of them.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.insert_member(&PlaylistMember::new(
                playlist_one,
                song_id,
                0,
                SongAvailability::Available,
            ))?;
            tx.insert_member(&PlaylistMember::new(
                playlist_two,
                song_id,
                0,
                SongAvailability::Available,
            ))
        }))
        .expect("multi-playlist add");
    assert_eq!(database.members(playlist_one).expect("one").len(), 1);
    assert_eq!(database.members(playlist_two).expect("two").len(), 1);

    // A failure on the second target rolls the first back as well: the song
    // joins playlist one (append) but hits an occupied position in playlist
    // two, so the whole transaction — including playlist one's membership —
    // must disappear.
    let song_c = song(root, "c.flac", "C", "丙");
    SongRepository::upsert(&database, &song_c).expect("c");
    let song_c_id = song_c.id();
    let result: Result<(), Error> = database.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        tx.insert_member(&PlaylistMember::new(
            playlist_one,
            song_c_id,
            u64::MAX,
            SongAvailability::Available,
        ))?;
        tx.insert_member(&PlaylistMember::new(
            playlist_two,
            song_c_id,
            0,
            SongAvailability::Available,
        ))
    }));
    assert!(result.is_err(), "position clash must fail the transaction");
    assert_eq!(
        database.members(playlist_one).expect("one").len(),
        1,
        "first membership of the failed transaction is rolled back"
    );
    assert_eq!(database.members(playlist_two).expect("two").len(), 1);
}

/// Task 5.3: the conditional unique target claim protects the import pipeline
/// against a live second claim on the same planned target — the claim blocks
/// until the blocking operation reaches a terminal state and releases it.
/// Register the import fixtures on the scan stack: the source content's tags
/// (content-keyed lookup) and the published path's probe/tags (path-keyed).
fn seed_import_source(
    stack: &ScanStack,
    published: &str,
    content: &[u8],
    artist: &str,
    title: &str,
) {
    let tags = |artist: &str, title: &str| crate::domain::media::ParsedMetadata {
        artist: Some(artist.to_owned()),
        title: Some(title.to_owned()),
        ..crate::domain::media::ParsedMetadata::default()
    };
    stack.metadata.set_bytes(content, tags(artist, title));
    stack.probe.set(
        published,
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(Duration::from_secs(1)),
        },
    );
    stack.metadata.set(published, tags(artist, title));
}

#[test]
fn import_target_claims_are_conditionally_unique_until_release() {
    let stack = ScanStack::new();

    // An in-flight operation holds the active claim on the planned target.
    let blocker = OperationId::new();
    OperationJournalRepository::ensure_operation(
        stack.database.as_ref(),
        blocker,
        stack.root,
        "import",
        None,
    )
    .expect("envelope");
    let claimed = RelativeMediaPath::new("歌手/歌手 - 晴天.flac").expect("path");
    stack
        .database
        .upsert_item(
            blocker,
            claim_item(None, claimed.display(), claimed.identity_key()),
        )
        .expect("active claim");

    // The import plans the same target (its batch snapshot predates the
    // claim): the conditional unique index refuses the second claim, so the
    // input fails without publishing anything.
    let content = b"claim-conflict-bytes";
    seed_import_source(&stack, claimed.display(), content, "歌手", "晴天");
    let deps = stack.deps();
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("hit", "晴天.flac", content);
    let source = crate::application::ports::ImportSource::new("hit").expect("key");
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(stack.root, std::slice::from_ref(&source))
        .expect("batch-level success");
    let crate::application::import::ImportOutcome::Failed { code, .. } = &report.results[0] else {
        panic!(
            "the live claim must block a second claim: {:?}",
            report.results[0]
        );
    };
    assert_eq!(
        *code, "conflict",
        "SQLite uniqueness surfaces as a conflict"
    );
    assert!(
        stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query")
            .items
            .is_empty(),
        "no song record while the claim is held by the blocker"
    );
    assert!(
        !stack.library_dir.join(claimed.display()).exists(),
        "nothing was published while the claim was held"
    );

    // After the blocker rolls back (claims released), the same import
    // succeeds under the same planned target with its own reserved identity.
    OperationJournalRepository::release_claims(stack.database.as_ref(), blocker).expect("release");
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(stack.root, &[source])
        .expect("batch-level success");
    let crate::application::import::ImportOutcome::Imported {
        song,
        target,
        operation,
        ..
    } = &report.results[0]
    else {
        panic!(
            "the import succeeds after the claim is released: {:?}",
            report.results[0]
        );
    };
    assert_eq!(target.display(), "歌手/歌手 - 晴天.flac");
    assert!(
        stack.library_dir.join(target.display()).is_file(),
        "the full audio is published"
    );
    assert!(
        crate::application::ports::SongRepository::by_id(stack.database.as_ref(), *song)
            .expect("query")
            .is_some(),
        "the record is committed under the reserved identity"
    );
    // The import's own claim was released at the terminal state.
    let items = stack.database.items(*operation).expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].state,
        crate::domain::state::OperationState::Completed
    );
    assert_eq!(
        items[0].source.as_deref(),
        Some("hit"),
        "the per-item source locator is persisted"
    );
    assert!(
        items[0].staging_path.is_some(),
        "the per-item staging location is persisted"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: scan pipeline against the real SQLite adapter (tasks 4.6/4.7/4.10)
// ---------------------------------------------------------------------------

use crate::application::ports::{
    CoverRepository, LibraryFileSystem as LibraryFileSystemPort, LyricsRepository, OperationItem,
    OperationJournalRepository, RuntimeStateStore, ScanRunRepository, TxWork,
};
use crate::application::root_switch::{
    ActivateLibrary, Blockers, PrepareLibraryCandidate, ROOT_EPOCH_KEY,
};
use crate::application::scan::{ScanConfig, ScanDeps, ScanSupervisor, StartScan};
use crate::application::testing::clock::{FakeIdGenerator, ManualClock, SteppingClock};
use crate::application::testing::filesystem::FakeLibraryFileSystem;
use crate::application::testing::small_fakes::{
    FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, FakeTrash,
};
use crate::application::trash::{finalize_persisted_trash, FinalizeExpiredDeletes};
use crate::domain::state::scan::ScanState as ScanRunState;
use crate::infrastructure::metadata::DiskCoverCache;

#[derive(Clone)]
struct RollbackAfterWrites {
    database: Arc<SqliteDatabase>,
}

impl UnitOfWork for RollbackAfterWrites {
    fn with_tx(&self, work: TxWork) -> Result<(), crate::error::Error> {
        self.database.with_tx(Box::new(move |tx| {
            work(tx)?;
            Err(crate::error::Error::unavailable(
                "database",
                "simulated transaction failure",
            ))
        }))
    }
}

/// A real SQLite database wired to fake file-system/metadata adapters — the
/// composition the desktop runtime will build, minus the OS pieces. One
/// `Arc<SqliteDatabase>` serves every repository port view, exactly like the
/// production composition root.
struct ScanStack {
    _directory: tempfile::TempDir,
    library_dir: std::path::PathBuf,
    database: Arc<SqliteDatabase>,
    fs: Arc<FakeLibraryFileSystem>,
    fs_port: Arc<dyn LibraryFileSystemPort>,
    probe: FakeMediaProbe,
    metadata: FakeMetadataReader,
    root: LibraryRootId,
}

impl ScanStack {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = Arc::new(
            SqliteDatabase::open(directory.path().join("echo.db")).expect("open database"),
        );
        let root = LibraryRootId::new();
        let library_dir = directory.path().join("library");
        std::fs::create_dir_all(&library_dir).expect("library dir");
        LibraryRepository::upsert(
            database.as_ref(),
            &LibraryRoot::new(root, library_dir.clone(), true, true),
        )
        .expect("insert active root");
        let fs = FakeLibraryFileSystem::with_root(root);
        fs.add_root_at(root, library_dir.clone());
        let fs_port: Arc<dyn LibraryFileSystemPort> = Arc::new(fs.clone());
        Self {
            _directory: directory,
            library_dir,
            database,
            fs: Arc::new(fs),
            fs_port,
            probe: FakeMediaProbe::new(),
            metadata: FakeMetadataReader::new(),
            root,
        }
    }

    fn deps(&self) -> ScanDeps {
        // One `Arc<SqliteDatabase>` coerced into every port view.
        let db: Arc<SqliteDatabase> = Arc::clone(&self.database);
        let db_root: Arc<dyn LibraryRepository> = db.clone();
        let db_songs: Arc<dyn SongRepository> = db.clone();
        let db_lyrics: Arc<dyn LyricsRepository> = db.clone();
        let db_covers: Arc<dyn CoverRepository> = db.clone();
        let db_runs: Arc<dyn ScanRunRepository> = db.clone();
        let db_journal: Arc<dyn OperationJournalRepository> = db.clone();
        let db_uow: Arc<dyn UnitOfWork> = db;
        ScanDeps {
            roots: db_root,
            songs: db_songs,
            lyrics: db_lyrics,
            covers: db_covers,
            runs: db_runs,
            journal: db_journal,
            uow: db_uow,
            fs: Arc::clone(&self.fs_port),
            probe: Arc::new(self.probe.clone()),
            metadata: Arc::new(self.metadata.clone()),
            hasher: Arc::new(FakeFileHasher::new(Arc::clone(&self.fs_port))),
            lyrics_parser: Arc::new(FakeLyricsParser::new()),
            cover_cache: Arc::new(
                DiskCoverCache::new(tempfile::TempDir::new().expect("cache dir").keep())
                    .expect("cover cache"),
            ),
            ids: Arc::new(FakeIdGenerator::new()),
            clock: Arc::new(ManualClock::new()),
            config: ScanConfig {
                batch_size: 2,
                worker_threads: 1,
                ..ScanConfig::default()
            },
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let absolute = self.library_dir.join(path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(absolute, bytes).expect("write file");
    }
}

fn seed_trash_operation(
    stack: &ScanStack,
    state: OperationState,
) -> (ScanDeps, Song, PlaylistId, OperationId) {
    let deps = stack.deps();
    let song = song(stack.root, "song.flac", "Song", "Artist");
    SongRepository::upsert(stack.database.as_ref(), &song).expect("seed song");
    SongRepository::set_availability(
        stack.database.as_ref(),
        song.id(),
        SongAvailability::PendingDelete,
    )
    .expect("hide pending song");
    let playlist = PlaylistId::new();
    stack
        .database
        .create(playlist, stack.root, "delete test")
        .expect("playlist");
    stack
        .database
        .add_member(playlist, song.id(), 0)
        .expect("member");

    let operation = OperationId::new();
    let staging = RelativeMediaPath::new(&format!("trash/{operation}/audio")).expect("stage path");
    stack.write(staging.display(), b"staged-song");
    let expected_hash = deps.hasher.hash(stack.root, &staging).expect("stage hash");
    OperationJournalRepository::ensure_operation(
        stack.database.as_ref(),
        operation,
        stack.root,
        crate::application::delete::DELETE_OPERATION,
        Some(song.id()),
    )
    .expect("operation envelope");
    OperationJournalRepository::upsert_item(
        stack.database.as_ref(),
        operation,
        OperationItem {
            kind: OperationResourceKind::Audio,
            state,
            song: Some(song.id()),
            source: None,
            staging_path: Some(staging),
            target_path: song.path().clone(),
            expected_hash,
            item_key: "audio".to_owned(),
            claim_key: "audio".to_owned(),
        },
    )
    .expect("journal item");
    (deps, song, playlist, operation)
}

fn claim_active(database: &SqliteDatabase, operation: OperationId) -> i64 {
    database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT claim_active FROM operation_items WHERE operation_uuid = ?1 AND item_key = 'audio'",
                    params![operation.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .expect("claim query")
}

#[test]
fn sqlite_trash_forward_finalizes_song_relationships_journal_and_claim() {
    let stack = ScanStack::new();
    let (deps, song, playlist, operation) =
        seed_trash_operation(&stack, OperationState::TrashPending);
    let trash = FakeTrash::new();

    let report = FinalizeExpiredDeletes::new(&deps, &trash)
        .run(stack.root)
        .expect("forward delete");

    assert_eq!(report.finalized, vec![operation]);
    assert_eq!(trash.calls(), vec![operation]);
    assert!(SongRepository::by_id(stack.database.as_ref(), song.id())
        .expect("song query")
        .is_none());
    assert!(stack
        .database
        .members(playlist)
        .expect("members")
        .is_empty());
    assert!(
        OperationJournalRepository::items(stack.database.as_ref(), operation)
            .expect("journal items")
            .iter()
            .all(|item| item.state == OperationState::DatabaseFinalized)
    );
    assert_eq!(claim_active(stack.database.as_ref(), operation), 0);
}

#[test]
fn sqlite_trash_finalization_rollback_keeps_song_journal_and_claim_together() {
    let stack = ScanStack::new();
    let (mut deps, song, playlist, operation) =
        seed_trash_operation(&stack, OperationState::TrashApplied);
    deps.uow = Arc::new(RollbackAfterWrites {
        database: Arc::clone(&stack.database),
    });

    let items = OperationJournalRepository::items(stack.database.as_ref(), operation)
        .expect("journal items");
    assert!(finalize_persisted_trash(&deps, operation, &items).is_err());

    let stored = SongRepository::by_id(stack.database.as_ref(), song.id())
        .expect("song query")
        .expect("rollback keeps song");
    assert_eq!(stored.availability(), SongAvailability::PendingDelete);
    assert_eq!(stack.database.members(playlist).expect("members").len(), 1);
    assert!(
        OperationJournalRepository::items(stack.database.as_ref(), operation)
            .expect("journal items")
            .iter()
            .all(|item| item.state == OperationState::TrashApplied)
    );
    assert_eq!(claim_active(stack.database.as_ref(), operation), 1);
}

#[test]
fn scan_pipeline_persists_songs_lyrics_covers_and_progress() {
    let stack = ScanStack::new();
    stack.write("a.mp3", b"audio-a");
    stack.write("华语/b.flac", b"audio-b");
    stack.write("华语/b.lrc", b"[00:01.00]sidecar");
    for (path, title) in [("a.mp3", "A"), ("华语/b.flac", "B")] {
        stack.probe.set(
            path,
            crate::application::ports::ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
                duration: Some(Duration::from_secs(1)),
            },
        );
        stack.metadata.set(
            path,
            crate::domain::media::ParsedMetadata {
                title: Some(title.to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(Duration::from_secs(1)),
                format: crate::domain::media::AudioFormat::Flac,
                embedded_lyrics: Some("[00:02.00]embedded".to_owned()),
                cover: Some(crate::domain::media::EmbeddedCover {
                    bytes: b"cover-".repeat(64),
                    mime: "image/png".to_owned(),
                }),
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
    }
    let stack_deps = stack.deps();
    let summary = StartScan::new(&stack_deps, &ScanSupervisor::new())
        .run(stack.root)
        .expect("scan ok");
    assert_eq!(summary.progress.created, 2);
    assert_eq!(summary.progress.state, ScanRunState::Completed);

    // Songs persisted with scan facts and visible to the catalog query.
    let page = stack
        .database
        .query_active_songs("", SongSort::default(), None, 100)
        .expect("query");
    assert_eq!(page.items.len(), 2);
    let song = page
        .items
        .iter()
        .find(|song| song.path().display() == "a.mp3")
        .expect("song");
    assert_eq!(song.title(), Some("A"));
    assert!(song.blake3_hash().is_some(), "scan facts persisted");
    assert_eq!(song.file_size(), Some(7));

    // Lyrics candidates per source, persisted transactionally with the song.
    let candidates = stack.database.candidates(song.id()).expect("candidates");
    assert!(candidates
        .iter()
        .any(|candidate| candidate.source() == crate::domain::entities::LyricsSource::Embedded));
    // The sidecar pairs with the *other* file (华语/b.flac + 华语/b.lrc).
    let song_b = page
        .items
        .iter()
        .find(|song| song.path().display() == "华语/b.flac")
        .expect("song b");
    let candidates_b = stack.database.candidates(song_b.id()).expect("candidates");
    let sources_b: Vec<_> = candidates_b
        .iter()
        .map(crate::domain::entities::LyricsCandidate::source)
        .collect();
    assert!(sources_b.contains(&crate::domain::entities::LyricsSource::Embedded));
    assert!(sources_b.contains(&crate::domain::entities::LyricsSource::Sidecar));

    // Cover reference + asset key consistency.
    let cover = stack
        .database
        .cover_of(song.id())
        .expect("cover")
        .expect("attached");
    assert!(cover.asset_key.starts_with("cv1-"));
    assert!(!stack
        .database
        .referenced_asset_keys(stack.root)
        .expect("keys")
        .is_empty());

    // Scan runs + issues persisted.
    let (state, progress, finished) = stack
        .database
        .scan_run_snapshot(stack.root, 1)
        .expect("run row")
        .expect("row");
    assert_eq!(state, ScanRunState::Completed);
    assert!(finished);
    assert_eq!(progress.discovered, 2);
    assert_eq!(
        stack.database.latest_generation(stack.root).expect("gen"),
        Some(1)
    );

    // Search works on the freshly scanned library (Unicode-safe path too).
    let page = stack
        .database
        .search_active_songs("歌手", SongSort::default(), None, 100)
        .expect("search");
    assert_eq!(page.items.len(), 2);
}

/// Probe double that stalls each probe so a scan stays in flight.
struct SlowProbe {
    inner: FakeMediaProbe,
    delay: Duration,
}

impl crate::application::ports::MediaProbe for SlowProbe {
    fn probe(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<crate::application::ports::ProbeOutcome, Error> {
        std::thread::sleep(self.delay);
        self.inner.probe(root, path)
    }
}

#[test]
fn scan_pipeline_keeps_ui_queries_available_during_scan() {
    let stack = ScanStack::new();
    for index in 0..20 {
        let path = format!("file{index}.mp3");
        stack.write(&path, format!("audio-{index}").as_bytes());
        stack.probe.set(
            &path,
            crate::application::ports::ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
                duration: Some(Duration::from_secs(1)),
            },
        );
        stack.metadata.set(
            &path,
            crate::domain::media::ParsedMetadata {
                title: Some(format!("T{index}")),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(Duration::from_secs(1)),
                format: crate::domain::media::AudioFormat::Flac,
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
    }
    // Slow the workers so the scan runs long enough to query against.
    let mut deps = stack.deps();
    deps.config.batch_size = 4;
    deps.config.worker_threads = 2;
    deps.probe = Arc::new(SlowProbe {
        inner: stack.probe.clone(),
        delay: Duration::from_millis(10),
    });

    let scan_deps = Arc::new(deps);
    let worker_deps = Arc::clone(&scan_deps);
    let supervisor = ScanSupervisor::new();
    let worker_supervisor = supervisor;
    let root = stack.root;
    let scanner =
        std::thread::spawn(move || StartScan::new(&worker_deps, &worker_supervisor).run(root));

    // While the scan is in flight, catalog queries keep succeeding (readers
    // are never blocked by the writer's small batches).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut queries = 0;
    while !scanner.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "scan did not finish in time"
        );
        let page = stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query during scan");
        assert!(page.items.len() <= 20);
        queries += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    let summary = scanner.join().unwrap().expect("scan ok");
    assert_eq!(summary.progress.created, 20);
    assert!(queries > 0, "queries ran while the scan was in flight");
    let final_page = stack
        .database
        .query_active_songs("", SongSort::default(), None, 100)
        .expect("query after scan");
    assert_eq!(final_page.items.len(), 20);
}

#[test]
fn scan_runs_progress_issues_round_trip_through_sqlite() {
    let stack = ScanStack::new();
    stack.write("good.mp3", b"good");
    stack.probe.set(
        "good.mp3",
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(Duration::from_secs(1)),
        },
    );
    stack.metadata.set(
        "good.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Good".to_owned()),
            duration: Some(Duration::from_secs(1)),
            format: crate::domain::media::AudioFormat::Flac,
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    // One bad file forms a persistent issue row.
    stack.write("bad.mp3", b"bad");
    stack.probe.set(
        "bad.mp3",
        crate::application::ports::ProbeOutcome::Unsupported,
    );

    // A stepping clock makes the throttle deterministic (60 ms steps vs a
    // 100 ms interval: batch snapshots are suppressed, terminal persists).
    let mut deps = stack.deps();
    deps.clock = Arc::new(SteppingClock::new(60));
    let summary = StartScan::new(&deps, &ScanSupervisor::new())
        .run(stack.root)
        .expect("scan ok");
    assert_eq!(summary.progress.failed, 1);

    let (state, progress, finished) = stack
        .database
        .scan_run_snapshot(stack.root, 1)
        .expect("run row")
        .expect("row");
    assert_eq!(state, ScanRunState::Completed);
    assert_eq!(progress.discovered, 2);
    assert_eq!(progress.failed, 1);
    assert!(finished, "terminal state never lost");
    let issues = stack.database.scan_issues(stack.root, 1).expect("issues");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code(), "unsupported_media");
    // The bad file did not block the good one.
    assert_eq!(
        stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query")
            .items
            .len(),
        1
    );
}

#[test]
fn activation_commits_active_root_and_epoch_atomically() {
    let stack = ScanStack::new();
    let second_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    let root_two =
        crate::application::root_switch::derive_root_id(&second_dir.path().canonicalize().unwrap());
    stack
        .fs
        .add_root_at(root_two, second_dir.path().to_path_buf());

    let deps = stack.deps();
    let supervisor = ScanSupervisor::new();
    // Prepare + activate the empty second root.
    let prepared = PrepareLibraryCandidate::new(&deps, stack.database.as_ref(), &supervisor)
        .prepare(second_dir.path())
        .expect("prepare");
    let outcome = ActivateLibrary::new(
        &deps,
        stack.database.as_ref(),
        &supervisor,
        stack.database.as_ref(),
        &Blockers::new(),
    )
    .activate(prepared.root_id)
    .expect("activate");
    assert_eq!(outcome.epoch.as_u64(), 1);
    // One transaction flipped active + epoch: both are visible now.
    let active = stack.database.active_root().expect("active").expect("root");
    assert_eq!(active.id(), root_two);
    assert_eq!(
        stack
            .database
            .load(ROOT_EPOCH_KEY)
            .expect("epoch")
            .as_deref(),
        Some("1")
    );
    // And the previous root record is kept but inactive.
    assert!(
        !LibraryRepository::by_id(stack.database.as_ref(), stack.root)
            .expect("by id")
            .unwrap()
            .is_active()
    );
}

// ---------------------------------------------------------------------------
// Task 6.1 — catalog view scenarios (全部歌曲 / 最近添加 / 喜欢的音乐 / 歌单)
// ---------------------------------------------------------------------------

/// Seed the active root with songs covering every view-relevant state.
/// Returns `(available, missing, pending_delete, favorite)` so tests can assert
/// membership of each view without re-deriving ids from the database.
fn seed_view_fixture(
    database: &SqliteDatabase,
    root: LibraryRootId,
) -> (Vec<Song>, Vec<Song>, Vec<Song>, Vec<Song>) {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    let mut pending_delete = Vec::new();
    let mut favorite = Vec::new();

    // Deterministic insertion order key (`added_at`) — the recent-view ladder.
    for (stamp, index) in (1000u64..).zip(0..6) {
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(&format!("songs/tune-{index}.flac")).expect("path"),
            Revision::INITIAL,
            stamp,
        );
        record.apply_metadata(
            Some(format!("title {index}")),
            Some(format!("artist {index}")),
            Some("album".to_owned()),
            Some(Duration::from_secs(180)),
        );
        if index == 2 {
            record.mark_missing();
            missing.push(record.clone());
        } else if index == 4 {
            record.set_favorite(true);
            available.push(record.clone());
            favorite.push(record.clone());
        } else if index == 5 {
            record.begin_pending_delete();
            pending_delete.push(record.clone());
        } else {
            available.push(record.clone());
        }
        SongRepository::upsert(database, &record).expect("seed song");
    }

    // A second, inactive root must never leak into any view.
    let other_root = LibraryRootId::new();
    let mut foreign = Song::new(
        SongId::new(),
        other_root,
        RelativeMediaPath::new("songs/foreign.flac").expect("path"),
        Revision::INITIAL,
    );
    foreign.apply_metadata(
        Some("foreign".to_owned()),
        Some("outsider".to_owned()),
        Some("album".to_owned()),
        Some(Duration::from_secs(180)),
    );
    // The inactive root's record must exist for the foreign song to be valid.
    let dir = tempfile::tempdir().expect("tempdir");
    LibraryRepository::upsert(
        database,
        &LibraryRoot::new(other_root, dir.path().join("other"), false, true),
    )
    .expect("insert inactive root");
    SongRepository::upsert(database, &foreign).expect("seed foreign song");

    (available, missing, pending_delete, favorite)
}

/// 全部歌曲: only the active root's available songs, pending-delete hidden,
/// other roots never visible.
#[test]
fn catalog_all_songs_view_covers_active_root_available_songs_only() {
    let (_directory, database, root) = database();
    let (expected, missing, pending_delete, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .all_songs(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("all songs");
    assert!(page.is_last);
    assert_eq!(page.next_cursor, None);

    let mut ids: Vec<_> = page.items.iter().map(Song::id).collect();
    let mut expected_ids: Vec<_> = expected.iter().map(Song::id).collect();
    ids.sort();
    expected_ids.sort();
    assert_eq!(ids, expected_ids, "available songs of the active root only");
    for record in &page.items {
        assert_ne!(record.id(), pending_delete[0].id(), "pending-delete hidden");
        assert_ne!(
            record.id(),
            missing[0].id(),
            "missing hidden from all-songs"
        );
        assert_eq!(record.availability(), SongAvailability::Available);
    }
    // Favorite songs still appear in 全部歌曲 (favorite is orthogonal).
    assert!(page.items.iter().any(|s| s.id() == favorite[0].id()));
}

/// 喜欢的音乐: favorites of the active root, keyset-stable, pending-delete
/// hidden, other roots never visible.
#[test]
fn catalog_favorites_view_is_favorited_available_active_root_songs_only() {
    let (_directory, database, root) = database();
    let (_available, _missing, _pending, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites");
    assert!(page.is_last);
    let mut ids: Vec<_> = page.items.iter().map(Song::id).collect();
    let mut favored_ids: Vec<_> = favorite.iter().map(Song::id).collect();
    ids.sort();
    favored_ids.sort();
    assert_eq!(
        ids, favored_ids,
        "exactly the favorited, available songs of the active root"
    );
    assert!(page.items.iter().all(Song::favorite));

    // Un-favoriting removes the song from the view immediately.
    SongRepository::set_favorite(&database, favorite[0].id(), false).expect("unfavorite");
    let after = query
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites after unfavorite");
    assert!(after.items.is_empty(), "取消收藏后歌曲立即从该视图移除");
}

/// 最近添加: the newest 100 available songs by added_at desc, stable UUID
/// tie-break, pending-delete + missing hidden, other roots never visible.
#[test]
fn catalog_recent_100_is_newest_available_with_stable_tie_break() {
    let (_directory, database, root) = database();
    let (_available, missing, pending_delete, _favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let recent = query.recent_100().expect("recent 100");
    // All six seeded songs have distinct added_at; the two non-available ones
    // (missing + pending-delete) are dropped, leaving four.
    assert_eq!(recent.len(), 4);
    let ids: Vec<_> = recent.iter().map(Song::id).collect();
    let missing_id = missing[0].id();
    let pending_id = pending_delete[0].id();
    assert!(!ids.contains(&missing_id), "missing hidden");
    assert!(!ids.contains(&pending_id), "pending-delete hidden");
    // Newest first on the added_at ladder.
    let stamps: Vec<_> = recent.iter().map(Song::added_at).collect();
    let mut sorted = stamps.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(stamps, sorted, "最新添加排在前面");

    // Stable tie-break: two records sharing an added_at keep deterministic order.
    let mut tied_a = Song::with_added_at(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/tied-a.flac").expect("path"),
        Revision::INITIAL,
        999,
    );
    let mut tied_b = Song::with_added_at(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/tied-b.flac").expect("path"),
        Revision::INITIAL,
        999,
    );
    tied_a.apply_metadata(
        Some("tied a".into()),
        Some("a".into()),
        Some("album".into()),
        Some(Duration::from_secs(1)),
    );
    tied_b.apply_metadata(
        Some("tied b".into()),
        Some("b".into()),
        Some("album".into()),
        Some(Duration::from_secs(2)),
    );
    SongRepository::upsert(&database, &tied_a).expect("tied a");
    SongRepository::upsert(&database, &tied_b).expect("tied b");
    let recent_again = query.recent_100().expect("recent again");
    assert_eq!(recent_again.len(), 6, "the two tied songs join the four");
    assert!(
        recent_again.iter().any(|s| s.id() == tied_a.id()),
        "tied a present"
    );
    assert!(
        recent_again.iter().any(|s| s.id() == tied_b.id()),
        "tied b present"
    );
    // It's deterministic: re-running reproduces the same relative order.
    let recent_thrice = query.recent_100().expect("recent thrice");
    let order_one: Vec<_> = recent_again.iter().map(Song::id).collect();
    let order_two: Vec<_> = recent_thrice.iter().map(Song::id).collect();
    assert_eq!(order_one, order_two, "重复刷新不得随机改变顺序");
}

/// 歌单: members by position over the active root; available + missing shown,
/// pending-delete hidden.
#[test]
fn catalog_playlist_view_shows_available_and_missing_hides_pending_delete() {
    let (_directory, database, root) = database();
    let (available_songs, missing_songs, pending_songs, _favorite) =
        seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, available_songs[0].id(), 0)
        .expect("member available");
    database
        .add_member(playlist, missing_songs[0].id(), 1)
        .expect("member missing");
    database
        .add_member(playlist, pending_songs[0].id(), 2)
        .expect("member pending");

    let songs = query.playlist(playlist).expect("playlist songs");
    let ids: Vec<_> = songs.iter().map(Song::id).collect();
    assert_eq!(ids.len(), 2, "pending-delete member hidden");
    assert_eq!(ids[0], available_songs[0].id(), "position 0 first");
    assert_eq!(ids[1], missing_songs[0].id(), "position 1 second");
    assert!(!ids.contains(&pending_songs[0].id()));

    // A missing member was already visible (missing rows display so a blocked
    // row can be shown); restoring it to available keeps it in place, and the
    // pending-delete member stays hidden throughout.
    SongRepository::set_availability(
        &database,
        missing_songs[0].id(),
        SongAvailability::Available,
    )
    .expect("restore missing");
    let after = query.playlist(playlist).expect("playlist after restore");
    assert_eq!(after.len(), 2, "still two visible members");
    assert_eq!(
        after[0].id(),
        available_songs[0].id(),
        "position order kept"
    );
    assert_eq!(
        after[1].id(),
        missing_songs[0].id(),
        "restored member in place"
    );
}

/// 全部歌曲 keyset pagination is stable and hidden songs never appear, and a
/// stale cursor (revision guarded) is rejected.
#[test]
fn catalog_all_songs_keyset_pages_stably_and_rejects_stale_cursor() {
    let (_directory, database, root) = database();
    let (expected, _missing, _pending, _favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    for field in SongSortField::ALL {
        for direction in [SortDirection::Asc, SortDirection::Desc] {
            let sort = SongSort { field, direction };
            let mut collected: Vec<SongId> = Vec::new();
            let mut cursor: Option<OpaqueCursor> = None;
            loop {
                let page = query.all_songs(sort, cursor.as_ref(), 2).expect("page");
                for record in &page.items {
                    assert_eq!(record.availability(), SongAvailability::Available);
                }
                let page_ids: Vec<_> = page.items.iter().map(Song::id).collect();
                collected.extend(page_ids);
                if page.is_last {
                    break;
                }
                cursor = page.next_cursor;
                assert!(cursor.is_some(), "non-last page must carry a cursor");
            }
            let mut expected_ids: Vec<_> = expected.iter().map(Song::id).collect();
            expected_ids.sort();
            collected.sort();
            assert_eq!(
                collected, expected_ids,
                "{field:?} {direction:?} covers active available only"
            );
        }
    }

    // A write after the cursor was minted invalidates it (revision guard).
    let first = query
        .all_songs(
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            1,
        )
        .expect("first page");
    let cursor = first.next_cursor.expect("cursor");
    let mut extra = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("songs/extra.flac").expect("path"),
        Revision::INITIAL,
    );
    extra.apply_metadata(
        Some("extra".into()),
        Some("artist".into()),
        Some("album".into()),
        Some(Duration::from_secs(1)),
    );
    SongRepository::upsert(&database, &extra).expect("insert extra");
    assert!(
        query
            .all_songs(
                SongSort {
                    field: SongSortField::Title,
                    direction: SortDirection::Asc,
                },
                Some(&cursor),
                1,
            )
            .is_err(),
        "stale cursor rejected"
    );
}
