use std::sync::Arc;
use std::time::Duration;

use rusqlite::params;

use super::*;
use crate::application::catalog::CatalogQuery;
use crate::application::ports::{
    DeviceIdProvider, LibraryRepository, OperationResourceKind, PlaylistRepository, SongRepository,
    SyncStateReader, UnitOfWork,
};
use crate::domain::catalog::{SongSort, SongSortField, SortDirection};
use crate::domain::entities::{LyricsLine, RootAvailability, SongAvailability};
use crate::domain::ids::Revision;
use crate::domain::state::OperationState;
use crate::error::Error;

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

/// 95th percentile of a latency sample set (sorted in place).
fn p95(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    // ceil(0.95 * n) - 1, clamped to the last sample.
    let ceil_95 = n.saturating_mul(19).saturating_add(19) / 20;
    let idx = ceil_95.saturating_sub(1).min(n - 1);
    samples[idx]
}

#[test]
fn initial_migration_has_required_tables_indexes_and_no_account_or_telemetry() {
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
        // 0005 device-01 sync-foundation: shape ready, behavior deferred.
        "tombstones",
        "sync_state",
        "sync_outbox",
        // 0006 portable-layout: device/HLC/member-UUID shape.
        "device_state",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
    // Anti-pattern tables must never appear: account/telemetry were never in
    // scope, and their presence would be a real privacy regression.
    assert!(!names
        .iter()
        .any(|name| name.contains("account") || name.contains("telemetry")));
    assert!(database.quick_check().is_ok());
}

#[test]
fn sync_payloads_carry_no_absolute_paths() {
    // 3.13 / sync-foundation R04: outbox payloads are the future syncable
    // carrier — they must carry only object UUIDs, root-relative paths and
    // fields, never the library root's absolute path or any machine-local path.
    let (directory, database, root) = database();
    let imported = song(root, "歌手/歌 - 甲.flac", "歌", "歌手");
    SongRepository::upsert(&database, &imported).expect("upsert");
    let library_abs = directory.path().to_string_lossy().to_string();
    database
        .with_reader(|connection| {
            let mut statement = connection
                .prepare("SELECT payload_json FROM sync_outbox")
                .map_err(storage)?;
            let payloads: Vec<String> = statement
                .query_map([], |row| row.get(0))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            assert!(
                !payloads.is_empty(),
                "an import must have prewritten outbox rows"
            );
            for payload in &payloads {
                assert!(
                    !payload.contains(library_abs.as_str()),
                    "outbox payload leaks the library root absolute path: {payload}"
                );
                assert!(
                    !payload.contains("/Users/"),
                    "outbox payload leaks a machine-local path: {payload}"
                );
            }
            Ok(())
        })
        .expect("read outbox payloads");
}

#[test]
fn sync_foundation_tables_are_ready_but_inert() {
    // 方向 A (3.10–3.14): the sync-foundation schema is *shape-ready* — the
    // tables exist, `schema_base='full'` is recorded — but nothing in 0.1.0
    // pushes or reads them remotely. Offline boundary stays.
    let (_directory, database, root) = database();
    let base: String = database
        .sync_state_load("schema_base")
        .expect("read sync state")
        .expect("0005 writes schema_base='full'");
    assert_eq!(base, "full");

    // Outbox prewrite on an ordinary import (task 3.11): a song write produces
    // one song outbox row with the full snapshot (relative path only), and the
    // same (object, revision) is never duplicated.
    let imported = song(root, "周杰伦/周杰伦 - 晴天.flac", "晴天", "周杰伦");
    SongRepository::upsert(&database, &imported).expect("import upsert");
    let rows = database
        .outbox_kind_count(super::sync::KIND_SONG)
        .expect("outbox rows");
    assert!(rows >= 1, "a song write must prewrite an outbox row");
}

#[test]
fn sync_foundation_prewrites_song_playlist_and_tombstone() {
    // 3.11 / 3.12: the five logic-change classes produce outbox rows (+revision)
    // and Echo deletion writes a tombstone, inside the same transaction. 0.1.0
    // keeps these local (nothing pushes); this test proves the *shape*.
    let (_directory, database, root) = database();
    let imported = song(root, "歌手/歌 - 甲.flac", "歌", "歌手");
    SongRepository::upsert(&database, &imported).expect("upsert");

    // Favorite is an independent syncable fact keyed by the song UUID.
    SongRepository::set_favorite(&database, imported.id(), true).expect("favorite");
    let song_rows = database
        .outbox_kind_count(super::sync::KIND_SONG)
        .expect("song outbox count");
    assert_eq!(song_rows, 1, "favorite does not overwrite the song fact");
    let favorite_rows = database
        .outbox_kind_count(super::sync::KIND_FAVORITE)
        .expect("favorite outbox count");
    assert_eq!(favorite_rows, 1, "favorite creates its own portable fact");

    // Playlist create + member add → playlist outbox rows with member excerpt.
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "歌单")
        .expect("create playlist");
    database
        .add_member(playlist, imported.id(), u64::MAX)
        .expect("add member");
    let playlist_rows = database
        .outbox_kind_count(super::sync::KIND_PLAYLIST)
        .expect("playlist outbox count");
    assert_eq!(playlist_rows, 2, "create + member = two playlist facts");

    // 3.12: Echo delete finalization writes a durable tombstone for the song.
    // (Reaching finalize normally needs the full trash forward-roll; here we
    // verify delete_song's tombstone contract via a transaction on the actor.)
    let doomed = imported.id();
    database
        .with_tx(Box::new(move |tx| tx.delete_song(doomed)))
        .expect("finalize delete");
    let song_rows_after = database
        .outbox_kind_count(super::sync::KIND_SONG)
        .expect("song outbox after delete");
    assert_eq!(
        song_rows_after, 2,
        "finalized delete appends one more song fact"
    );
    let song_tombstoned = song_tombstone_count(&database);
    assert_eq!(
        song_tombstoned, 1,
        "Echo delete writes a durable song tombstone"
    );
}

/// Read the tombstone count for song objects through a reader probe (test-only
/// shape check; 0.1.0 never consumes tombstones).
fn song_tombstone_count(database: &SqliteDatabase) -> i64 {
    database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tombstones WHERE object_type = 'song'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(super::support::storage)
        })
        .expect("tombstone count")
}

fn outbox_revisions(database: &SqliteDatabase, kind: &str, uuid: &str) -> Vec<i64> {
    database
        .with_reader(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT revision FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2 ORDER BY revision",
                )
                .map_err(super::support::storage)?;
            let rows = statement
                .query_map(params![kind, uuid], |row| row.get::<_, i64>(0))
                .map_err(super::support::storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(super::support::storage)?;
            Ok(rows)
        })
        .expect("outbox revisions")
}

fn outbox_latest_payload(database: &SqliteDatabase, kind: &str, uuid: &str) -> String {
    database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT payload_json FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2 ORDER BY revision DESC LIMIT 1",
                    params![kind, uuid],
                    |row| row.get::<_, String>(0),
                )
                .map_err(super::support::storage)
        })
        .expect("latest payload")
}

/// Task 1.3: consecutive writes to the same object yield strictly monotone
/// revisions, and the outbox payload reflects the full current object state.
/// Favorites are independent portable facts, so their revision history must
/// not alter the song object's media-metadata history.
#[test]
fn sync_object_revisions_are_monotone_and_payloads_are_full() {
    let (_directory, database, root) = database();
    let imported = song(root, "歌手/歌 - 甲.flac", "歌", "歌手");
    SongRepository::upsert(&database, &imported).expect("upsert");
    let id = imported.id().to_string();

    // Favorite → its own first revision and a payload with no local path or
    // runtime state. The imported song remains at revision one.
    SongRepository::set_favorite(&database, imported.id(), true).expect("favorite");
    let revisions = outbox_revisions(&database, super::sync::KIND_SONG, &id);
    assert_eq!(
        revisions,
        vec![1],
        "favorite does not overwrite song object"
    );
    let favorite_revisions = outbox_revisions(&database, super::sync::KIND_FAVORITE, &id);
    assert_eq!(
        favorite_revisions,
        vec![1],
        "favorite starts its own revision"
    );
    let latest = outbox_latest_payload(&database, super::sync::KIND_FAVORITE, &id);
    let parsed: serde_json::Value = serde_json::from_str(&latest).expect("payload json");
    assert_eq!(parsed["is_favorite"], serde_json::Value::Bool(true));
    assert_eq!(parsed["song_uuid"], serde_json::Value::String(id.clone()));
    assert!(parsed.get("relative_path").is_none());
    assert!(parsed.get("play_count").is_none());

    // Un-favorite → the favorite object's second revision, monotone continues.
    SongRepository::set_favorite(&database, imported.id(), false).expect("unfavorite");
    let revisions = outbox_revisions(&database, super::sync::KIND_FAVORITE, &id);
    assert_eq!(revisions, vec![1, 2], "monotone across consecutive writes");

    // Playlist: create → member add, both monotone and full.
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "通勤")
        .expect("create playlist");
    database
        .add_member(playlist, imported.id(), u64::MAX)
        .expect("add member");
    let pl = playlist.to_string();
    let pl_revisions = outbox_revisions(&database, super::sync::KIND_PLAYLIST, &pl);
    assert_eq!(pl_revisions, vec![1, 2], "playlist revisions monotone");
    let pl_payload = outbox_latest_payload(&database, super::sync::KIND_PLAYLIST, &pl);
    let parsed: serde_json::Value = serde_json::from_str(&pl_payload).expect("payload json");
    assert!(
        parsed["members"].as_array().is_some_and(|m| m.len() == 1),
        "member payload carries the full member set"
    );
    // The member excerpt now carries a stable member UUID (task 1.2 mapping).
    let member = parsed["members"][0].clone();
    assert!(
        member.get("member_uuid").is_some(),
        "member payload carries member_uuid"
    );
}

/// Task 1.3: availability changes are syncable facts carrying the full object
/// snapshot, with a monotone revision (a scan marking a song missing must not
/// produce a duplicate/regressed revision).
#[test]
fn sync_availability_change_is_a_full_outbox_fact() {
    let (_directory, database, root) = database();
    let imported = song(root, "歌手/歌 - 甲.flac", "歌", "歌手");
    SongRepository::upsert(&database, &imported).expect("upsert");
    let id = imported.id().to_string();

    SongRepository::set_availability(&database, imported.id(), SongAvailability::Missing)
        .expect("mark missing");
    let revisions = outbox_revisions(&database, super::sync::KIND_SONG, &id);
    assert_eq!(
        revisions,
        vec![1, 2],
        "availability is a second monotone fact"
    );
    let payload = outbox_latest_payload(&database, super::sync::KIND_SONG, &id);
    assert!(payload.contains("miss"), "payload reflects missing state");
    assert!(payload.contains("relative_path"), "full payload present");
}

/// Task 1.3: concurrent sequential submissions for the same object (via the
/// single-writer actor) never interleave or produce duplicate revisions.
#[test]
fn sync_concurrent_sequential_submissions_stay_monotone() {
    let (_directory, database, root) = database();
    let imported = song(root, "歌手/歌 - 甲.flac", "歌", "歌手");
    SongRepository::upsert(&database, &imported).expect("upsert");
    let id = imported.id();

    std::thread::scope(|scope| {
        let database = &database;
        for i in 0..8 {
            scope.spawn(move || {
                // Alternate favorite on/off — each is a separate mutation, each
                // enqueues a revision.
                if i % 2 == 0 {
                    SongRepository::set_favorite(database, id, true).expect("fav on");
                } else {
                    SongRepository::set_favorite(database, id, false).expect("fav off");
                }
            });
        }
    });

    let id = id.to_string();
    let revisions = outbox_revisions(&database, super::sync::KIND_FAVORITE, &id);
    // Eight concurrent toggles become eight ordered favorite facts, no gaps.
    assert_eq!(revisions.len(), 8, "8 facts, no lost/duplicate revisions");
    let expected: Vec<i64> = (1..=8).collect();
    assert_eq!(
        revisions, expected,
        "revisions are strictly 1..9 with no gaps or duplicates"
    );
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

/// Migration 0006 adds the portable-layout shape: a device id store, HLC
/// columns on syncable objects, and a stable per-member UUID on playlist
/// members.
#[test]
fn migration_0006_portable_layout_columns_exist() {
    let (directory, _database, _) = database();
    // A fresh DB runs 0001..=0006. Probe the 0006 additions directly.
    let mut direct = open_writer(&directory.path().join("plain.db")).expect("connection");
    apply_migrations(&mut direct).expect("all migrations");
    direct
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('songs') WHERE name = 'hlc_wall_secs'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("songs.hlc_wall_secs");
    direct
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('playlist_songs') WHERE name = 'member_uuid'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("playlist_songs.member_uuid");
    direct
        .query_row(
            "SELECT value FROM device_state WHERE key = 'device_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("device_state seeded");
    drop(direct);
}

/// A database created at migration 0005 (a "pre-portable" install) upgrades
/// cleanly through 0006 without losing data or violating the immutable released
/// migrations.
#[test]
fn migration_0006_upgrades_a_migration_0005_database() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("old.db");
    let mut direct = open_writer(&path).expect("connection");
    // Ensure schema_migrations table exists (normally done by apply_migrations).
    direct
        .execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL);")
        .expect("create schema_migrations");
    // Apply 1..=5 only, then prove 0006 applies on top.
    let set: Vec<(i64, &str)> = vec![
        (1, include_str!("migrations/0001_initial.sql")),
        (2, include_str!("migrations/0002_library_assets.sql")),
        (3, include_str!("migrations/0003_root_write_safety.sql")),
        (4, include_str!("migrations/0004_song_audio_parameters.sql")),
        (5, include_str!("migrations/0005_sync_foundation.sql")),
    ];
    apply_migration_set(&mut direct, &set).expect("1..=5 apply");
    // A song + playlist member from the pre-portable era (no member_uuid yet).
    direct
        .execute_batch(
            "INSERT INTO library_roots (uuid, absolute_path, normalized_path_key, is_active, write_capable, availability, created_at, updated_at)
                VALUES ('root1', '/lib/root', '/lib/root', 1, 1, 'available', 1, 1);
             INSERT INTO songs (uuid, library_root_uuid, relative_path, normalized_relative_path, title, title_sort, artist_sort, album_sort, added_at, availability, created_at, updated_at)
                VALUES ('song1', 'root1', '歌手/歌.flac', '歌手/歌.flac', '歌', '歌', '歌手', '', 1, 'available', 1, 1);
             INSERT INTO playlists (uuid, library_root_uuid, display_name, normalized_name_key, created_at, updated_at)
                VALUES ('pl1', 'root1', '列表', '列表', 1, 1);
             INSERT INTO playlist_songs (playlist_uuid, song_uuid, position, added_at)
                VALUES ('pl1', 'song1', 0, 1);",
        )
        .expect("seed pre-portable data");
    apply_migrations(&mut direct).expect("upgrade to 0006");

    // The backfill gave the pre-existing member a stable UUID.
    let member_uuid: Option<String> = direct
        .query_row(
            "SELECT member_uuid FROM playlist_songs WHERE playlist_uuid = 'pl1' AND song_uuid = 'song1'",
            [],
            |row| row.get(0),
        )
        .expect("member row");
    assert!(
        member_uuid.is_some_and(|value| !value.is_empty()),
        "pre-existing member gets a stable UUID on upgrade"
    );
    // The upgrade did not modify released migrations.
    let checksums = direct
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version IN (1,2,3,4,5) AND checksum <> ''",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("checksums");
    assert_eq!(checksums, 5, "all five released migrations recorded");
    drop(direct);
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
    let original_member = database.members(playlist).expect("member")[0].id();
    // The same song in a different playlist is a distinct logical membership,
    // so it gets a distinct UUID rather than inheriting the song identity.
    let other_playlist = PlaylistId::new();
    database
        .create(other_playlist, root, "另一张歌单")
        .expect("other playlist");
    database
        .add_member(other_playlist, song_a.id(), u64::MAX)
        .expect("member in other playlist");
    assert_ne!(
        original_member,
        database.members(other_playlist).expect("other member")[0].id()
    );
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
    let repeated = database.members(playlist).expect("member after re-add")[0].clone();
    assert_eq!(repeated.id(), original_member, "member UUID remains stable");
    assert_eq!(repeated.position(), 0, "re-add preserves its append order");
    database
        .add_member(playlist, other.id(), u64::MAX)
        .expect("append c at 1");

    let members = database.members(playlist).expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].song(), song_a.id());
    assert_eq!(members[0].position(), 0);
    assert_eq!(members[1].song(), other.id());
    assert_eq!(members[1].position(), 1);

    database
        .remove_member(playlist, song_a.id())
        .expect("remove member");
    let tombstoned: i64 = database
        .with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tombstones WHERE object_type = 'playlist-item' AND object_uuid = ?1",
                    [original_member.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .expect("playlist-item tombstone query");
    assert_eq!(
        tombstoned, 1,
        "removal retains a recovery-consumable tombstone"
    );
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
    let claimed = RelativeMediaPath::new("media/歌手/歌手 - 晴天.flac").expect("path");
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
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
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
    MemoryControlPlane,
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
        let db_catalog: Arc<dyn CatalogQueryRepository> = db.clone();
        let db_playlists: Arc<dyn PlaylistRepository> = db.clone();
        let db_lyrics: Arc<dyn LyricsRepository> = db.clone();
        let db_covers: Arc<dyn CoverRepository> = db.clone();
        let db_runs: Arc<dyn ScanRunRepository> = db.clone();
        let db_journal: Arc<dyn OperationJournalRepository> = db.clone();
        let db_uow: Arc<dyn UnitOfWork> = db.clone();
        let db_device: Arc<dyn DeviceIdProvider> = db.clone();
        let db_sync: Arc<dyn SyncStateReader> = db;
        ScanDeps {
            roots: db_root,
            songs: db_songs,
            catalog: db_catalog,
            playlists: db_playlists,
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
            control: Arc::new(MemoryControlPlane::new()),
            device_id: db_device,
            sync: db_sync,
            clock: Arc::new(ManualClock::new()),
            config: ScanConfig {
                batch_size: 2,
                worker_threads: 1,
                ..ScanConfig::default()
            },
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        // The fake filesystem's `enumerate` only discovers `media/` (mirroring
        // the real walker, portable layout §5), so seeds land under `media/`.
        let absolute = if path.starts_with("media/") {
            self.library_dir.join(path)
        } else {
            self.library_dir.join(format!("media/{path}"))
        };
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
    let song = song(stack.root, "media/song.flac", "Song", "Artist");
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
    let staging =
        RelativeMediaPath::new(&format!("media/trash/{operation}/audio")).expect("stage path");
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
        .find(|song| song.path().display() == "media/a.mp3")
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
        .find(|song| song.path().display() == "media/华语/b.flac")
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

#[test]
fn catalog_favorites_orders_by_the_latest_favorite_action() {
    let (_directory, database, root) = database();
    let first = song(root, "songs/first.flac", "先收藏", "艺人");
    let last = song(root, "songs/last.flac", "后收藏", "艺人");
    SongRepository::upsert(&database, &first).expect("seed first");
    SongRepository::upsert(&database, &last).expect("seed last");
    SongRepository::set_favorite(&database, first.id(), true).expect("favorite first");
    SongRepository::set_favorite(&database, last.id(), true).expect("favorite last");

    // 最近添加 in the favorites view means the favorite action time, not the
    // song's original library insertion time.
    let page = CatalogQuery::new(&database)
        .favorites(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Desc,
            },
            None,
            100,
        )
        .expect("favorites");

    assert_eq!(
        page.items.iter().map(Song::id).collect::<Vec<_>>(),
        vec![last.id(), first.id()],
        "the song liked last is the first favorite row"
    );
}

#[test]
fn catalog_favorites_honors_its_own_manual_sort() {
    let (_directory, database, root) = database();
    let alphabetically_first = song(root, "songs/first.flac", "A song", "艺人");
    let alphabetically_last = song(root, "songs/last.flac", "Z song", "艺人");
    SongRepository::upsert(&database, &alphabetically_first).expect("seed first");
    SongRepository::upsert(&database, &alphabetically_last).expect("seed last");
    SongRepository::set_favorite(&database, alphabetically_first.id(), true).expect("favorite first");
    SongRepository::set_favorite(&database, alphabetically_last.id(), true).expect("favorite last");

    let page = CatalogQuery::new(&database)
        .favorites(
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("favorites by title");

    assert_eq!(
        page.items.iter().map(Song::id).collect::<Vec<_>>(),
        vec![alphabetically_first.id(), alphabetically_last.id()],
        "a favorites-only choice must not inherit the default recent-favorite order"
    );
}

/// 资料库导航计数: the SQLite implementation must agree with the views it
/// advertises *and* with the in-memory fake, so a count can never be an
/// artefact of one backend. The fixture is deliberately small enough that the
/// answer is "4 / 1 / 4" by inspection.
#[test]
fn catalog_counts_match_view_membership_over_sqlite() {
    let (_directory, database, root) = database();
    let (available, _missing, _pending_delete, favorite) = seed_view_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let counts = query.counts().expect("counts");
    assert_eq!(
        counts.all,
        available.len(),
        "全部歌曲 counts the active root's available songs"
    );
    assert_eq!(
        counts.favorites,
        favorite.len(),
        "喜欢的音乐 counts favorites"
    );
    assert_eq!(counts.recent, counts.all, "recent is uncapped below 100");

    // Cross-check against the views themselves, then against a mutation.
    assert_eq!(
        query
            .all_songs(SongSort::default(), None, 100)
            .expect("all")
            .items
            .len(),
        counts.all,
        "count agrees with the rendered 全部歌曲 rows"
    );

    // Toggle a favorite: the count must follow the commit.
    let target = available
        .iter()
        .find(|song| !song.favorite())
        .expect("a non-favorited available song");
    SongRepository::set_favorite(&database, target.id(), true).expect("favorite");
    let after = query.counts().expect("counts after favorite");
    assert_eq!(after.favorites, counts.favorites + 1, "收藏后计数 +1");
    assert_eq!(after.all, counts.all, "收藏不改变全部歌曲总数");
}

/// No active root is an absent library, not an empty one: returning
/// `{0, 0, 0}` would have the sidebar print "0" next to views it cannot open.
#[test]
fn catalog_counts_are_unavailable_without_an_active_root() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open");
    assert!(matches!(
        CatalogQuery::new(&database).counts(),
        Err(Error::Unavailable { .. })
    ));
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

// ---------------------------------------------------------------------------
// Task 6.2 — 资料库搜索：标题/艺人/专辑全体词包含、随视图叠加、清空恢复
// ---------------------------------------------------------------------------

/// Seed a deterministically-titled active root for search scenarios. Returns
/// the songs in (title, artist, album) order so expectations are explicit.
fn seed_search_fixture(
    database: &SqliteDatabase,
    root: LibraryRootId,
) -> Vec<(SongId, String, String, String)> {
    let rows = vec![
        ("晴天.mp3", "晴天", "周杰伦", "叶惠美"),
        ("晴天吉它版.flac", "晴天 (吉它版)", "杰倫", "叶惠美"),
        ("七里香.flac", "七里香", "周杰伦", "七里香"),
        ("晴天remix.flac", "Sunny Day Remix", "Jay", "叶惠美"),
    ];
    let mut out = Vec::new();
    for (index, (path, title, artist, album)) in rows.into_iter().enumerate() {
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(path).expect("path"),
            Revision::INITIAL,
            5000 + index as u64,
        );
        record.apply_metadata(
            Some(title.to_owned()),
            Some(artist.to_owned()),
            Some(album.to_owned()),
            Some(Duration::from_secs(200)),
        );
        if index == 0 {
            record.set_favorite(true);
        }
        SongRepository::upsert(database, &record).expect("seed song");
        out.push((
            record.id(),
            title.to_owned(),
            artist.to_owned(),
            album.to_owned(),
        ));
    }
    out
}

/// 搜索多个字段：完整查询词在标题、艺人、专辑中做不区分大小写包含匹配，
/// 至少返回一个字段包含完整查询词（而非碎片 token）的歌曲。
#[test]
fn catalog_search_matches_full_query_word_ignoring_case_across_fields() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    // 标题前缀 + 大小写不敏感（lat 匹配 "Sunny Day Remix" 于 "SunnyDay" 之流）
    let page = query
        .search(
            "sunny",
            false,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search sunny");
    assert_eq!(page.items.len(), 1, "one candidate matches on title");
    assert_eq!(
        page.items[0].id(),
        rows[3].0,
        "全查询词 'sunny' 命中 'Sunny Day Remix'"
    );

    // 艺人包含：“杰” -> 晴天(周杰伦)/七里香(周杰伦)/晴天吉它版(杰倫) 的艺人含
    // “杰”（NFKC 兼容折叠后“杰倫”仍含“杰”）；晴天 remix 艺人 "Jay" 不含。
    let page = query
        .search(
            "杰",
            false,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search 杰");
    let ids: Vec<_> = page.items.iter().map(Song::id).collect();
    assert!(ids.contains(&rows[0].0), "晴天 艺人 周杰伦");
    assert!(ids.contains(&rows[2].0), "七里香 艺人 周杰伦");
    assert!(ids.contains(&rows[1].0), "晴天吉它版 艺人 杰倫");
    assert!(!ids.contains(&rows[3].0), "remix 艺人 Jay 不含");

    // 专辑字段：“叶惠美” -> 三条（晴天、吉它版、remix），不含七里香。
    let page = query
        .search(
            "叶惠美",
            false,
            SongSort {
                field: SongSortField::Title,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("search album");
    assert_eq!(page.items.len(), 3);
}

/// 空查询词恢复当前视图完整集合（全部歌曲）。
#[test]
fn catalog_search_empty_query_restores_full_view() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let cleared = query
        .search(
            "",
            false,
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("empty search");
    assert_eq!(cleared.items.len(), rows.len(), "清空恢复完整集合");

    let full = query
        .all_songs(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("all songs");
    let cleared_ids: Vec<_> = cleared.items.iter().map(Song::id).collect();
    let full_ids: Vec<_> = full.items.iter().map(Song::id).collect();
    assert_eq!(cleared_ids, full_ids, "空搜索 == 当前视图完整集合与顺序");
}

/// 搜索无结果：返回空页，不报错，不吞 active 根问题。
#[test]
fn catalog_search_no_results_returns_empty_page_not_error() {
    let (_directory, database, root) = database();
    seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let page = query
        .search("不存在的歌名xyz", false, SongSort::default(), None, 100)
        .expect("no-results is not an error");
    assert!(page.items.is_empty());
    assert!(page.is_last);
    assert_eq!(page.next_cursor, None);
}

/// 搜索叠加在喜欢的音乐视图：只返回收藏且匹配的可用歌曲。
#[test]
fn catalog_search_overlays_on_favorites_view() {
    let (_directory, database, root) = database();
    let rows = seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    // 全视图搜索 "周" -> 三条（晴天、七里香 艺人周杰伦；吉它版艺人杰倫不含；remix Jay 不含）
    let all = query
        .search("周", false, SongSort::default(), None, 100)
        .expect("search 周");
    assert!(all.items.iter().any(|s| s.id() == rows[0].0));
    assert!(all.items.iter().any(|s| s.id() == rows[2].0));

    // 叠加 favorites：只有 favorite 的晴天命中。
    let fav = query
        .search("周", true, SongSort::default(), None, 100)
        .expect("search 周 in favorites");
    assert_eq!(fav.items.len(), 1);
    assert_eq!(fav.items[0].id(), rows[0].0);

    // 清空叠加 favorites：= favorites 视图。
    let cleared_fav = query
        .search(
            "",
            true,
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Asc,
            },
            None,
            100,
        )
        .expect("empty in favorites");
    assert_eq!(cleared_fav.items.len(), 1);
}

/// 过期请求取消（revision guard）：搜索会话中发生写入使 cursor 失效后，
/// 继续分页必须被拒绝，而非返回陈旧/错乱结果。
#[test]
fn catalog_search_stale_request_cancelled_after_write_invalidation() {
    let (_directory, database, root) = database();
    seed_search_fixture(&database, root);
    let query = CatalogQuery::new(&database);

    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    let first = query.search("", false, sort, None, 2).expect("page 1");
    let cursor = first.next_cursor.expect("non-last page cursor");
    assert!(!first.is_last);

    // A write bumps the root revision, invalidating the in-flight search.
    let mut later = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("later.flac").expect("path"),
        Revision::INITIAL,
    );
    later.apply_metadata(
        Some("later".to_owned()),
        Some("歌手".to_owned()),
        Some("专辑".to_owned()),
        Some(Duration::from_secs(1)),
    );
    SongRepository::upsert(&database, &later).expect("write bumps revision");
    assert!(
        query.search("", false, sort, Some(&cursor), 2).is_err(),
        "过期请求取消：revision 变化后 cursor 必须被拒绝"
    );
}

/// 50k 正确性：大量歌曲搜索分页稳定、不截断、每一页 keyset 正确覆盖。
#[test]
fn catalog_search_pages_deterministically_across_large_library() {
    let (_directory, database, root) = database();
    // 55 songs all titled with a shared token + distinct suffix.
    let mut all_ids = Vec::new();
    for index in 0..55u64 {
        let shared = index % 3 == 0; // every third song carries the search token
        let title = if shared {
            format!("共歌曲 {index:03}")
        } else {
            format!("獨歌 {index:03}")
        };
        let mut record = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new(&format!("songs/{index:03}.flac")).expect("path"),
            Revision::INITIAL,
            10_000 + index,
        );
        record.apply_metadata(
            Some(title.clone()),
            Some("大众".to_owned()),
            Some("专辑".to_owned()),
            Some(Duration::from_secs(1)),
        );
        SongRepository::upsert(&database, &record).expect("seed");
        all_ids.push(record.id());
    }

    let query = CatalogQuery::new(&database);
    let sort = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Asc,
    };

    // All-songs just to know the full count and that pagination works.
    let mut ids = Vec::new();
    let mut cursor: Option<OpaqueCursor> = None;
    loop {
        let page = query
            .search("", false, sort, cursor.as_ref(), 7)
            .expect("page");
        for r in &page.items {
            ids.push(r.id());
        }
        if page.is_last {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(
        ids.len(),
        55,
        "55 songs paged completely, nothing truncated"
    );
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 55, "no duplicate rows across pages");

    // Searching "共歌曲" hits exactly the 19 shared-token songs, deterministic.
    let mut hits = Vec::new();
    let mut cursor = None;
    loop {
        let page = query
            .search("共歌曲", false, sort, cursor.as_ref(), 5)
            .expect("search page");
        for r in &page.items {
            hits.push(r.id());
        }
        if page.is_last {
            break;
        }
        cursor = page.next_cursor;
    }
    let expected_count = all_ids
        .iter()
        .enumerate()
        .filter(|(i, _)| *i % 3 == 0)
        .count();
    assert_eq!(hits.len(), expected_count, "exactly the 19 candidates");
    // Determinism: repeat paging yields identical order.
    let repeat = {
        let mut ids = Vec::new();
        let mut cursor = None;
        loop {
            let page = query
                .search("共歌曲", false, sort, cursor.as_ref(), 5)
                .expect("repeat");
            for r in &page.items {
                ids.push(r.id());
            }
            if page.is_last {
                break;
            }
            cursor = page.next_cursor;
        }
        ids
    };
    assert_eq!(hits, repeat, "重复分页顺序必须确定性");
}

// ---------------------------------------------------------------------------
// Task 6.7 — playlist missing/blocked member display + Echo-delete cascade
// ---------------------------------------------------------------------------

/// 外部失效成员保留展示：missing 成员（外部删除）仍显示在歌单中并标记
/// 失效；同 UUID 恢复后重新可用且不产生重复成员。
#[test]
fn playlist_missing_members_stay_visible_and_recover_without_duplicates() {
    let (_directory, database, root) = database();
    let keep = song(root, "keep.flac", "Kept", "甲");
    let external = song(root, "gone.flac", "Gone", "乙");
    SongRepository::upsert(&database, &keep).expect("keep");
    SongRepository::upsert(&database, &external).expect("external");

    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, keep.id(), u64::MAX)
        .expect("keep member");
    database
        .add_member(playlist, external.id(), u64::MAX)
        .expect("external member");

    // External program deletion -> song Missing, membership retained + mirrored.
    SongRepository::set_availability(&database, external.id(), SongAvailability::Missing)
        .expect("mark missing");
    let members = database.members(playlist).expect("members");
    assert_eq!(members.len(), 2, "失效成员不得被移除");
    let external_row = members
        .iter()
        .find(|m| m.song() == external.id())
        .expect("member present");
    assert_eq!(
        external_row.song_availability(),
        SongAvailability::Missing,
        "member is marked unavailable"
    );

    // The playlist view still includes it (available + missing shown).
    let view = playlist_songs_view(&database, playlist);
    assert_eq!(view.len(), 2);
    assert!(view.iter().any(|s| s.id() == external.id()));

    // Same UUID restores: availability back to Available, one row only.
    SongRepository::set_availability(&database, external.id(), SongAvailability::Available)
        .expect("restore");
    let after = database.members(playlist).expect("members");
    assert_eq!(after.len(), 2, "no duplicate member on recovery");
    let restored = after
        .iter()
        .find(|m| m.song() == external.id())
        .expect("present");
    assert_eq!(
        restored.song_availability(),
        SongAvailability::Available,
        "失效标记取消"
    );
}

/// Echo 主动删除 finalize 级联移除成员：`delete_song`（finalize 路径）在同一
/// 事务内把歌曲和它所有歌单成员删除，其他歌单/歌曲不受影响且顺序保留。
#[test]
fn echo_delete_finalize_cascades_memberships_atomically() {
    let (_directory, database, root) = database();
    let doomed = song(root, "doomed.flac", "Doomed", "甲");
    let survivor = song(root, "survivor.flac", "Survivor", "乙");
    SongRepository::upsert(&database, &doomed).expect("doomed");
    SongRepository::upsert(&database, &survivor).expect("survivor");

    let first = PlaylistId::new();
    let second = PlaylistId::new();
    database.create(first, root, "一").expect("first");
    database.create(second, root, "二").expect("second");
    database
        .add_member(first, doomed.id(), u64::MAX)
        .expect("doomed in 一");
    database
        .add_member(first, survivor.id(), u64::MAX)
        .expect("survivor in 一");
    database
        .add_member(second, doomed.id(), u64::MAX)
        .expect("doomed in 二");

    // Echo delete hides first (pending-delete) — memberships stay.
    SongRepository::set_availability(&database, doomed.id(), SongAvailability::PendingDelete)
        .expect("pending delete");
    assert_eq!(database.members(first).expect("一").len(), 2);
    assert_eq!(database.members(second).expect("二").len(), 1);
    let hidden_view = playlist_songs_view(&database, first);
    assert!(
        !hidden_view.iter().any(|s| s.id() == doomed.id()),
        "pending-delete member hidden from the playlist view"
    );

    // The finalize step (as finalize_persisted_trash runs) deletes the song
    // row; the FK cascades remove its membership rows in the same transaction.
    let doomed_id = doomed.id();
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.delete_song(doomed_id)
        }))
        .expect("finalize delete");
    assert!(SongRepository::by_id(&database, doomed.id())
        .expect("query")
        .is_none());
    let first_after = database.members(first).expect("一 after");
    assert_eq!(
        first_after.len(),
        1,
        "cascade removed the doomed membership, survivor stays"
    );
    assert_eq!(first_after[0].song(), survivor.id());
    assert_eq!(database.members(second).expect("二 after").len(), 0);
    // The survivor song itself is untouched.
    assert!(SongRepository::by_id(&database, survivor.id())
        .expect("query")
        .is_some());
}

/// Direct read of the playlist view (same contract as `playlist_songs_query`).
fn playlist_songs_view(database: &SqliteDatabase, playlist: PlaylistId) -> Vec<Song> {
    use crate::application::catalog::CatalogQuery;
    CatalogQuery::new(database)
        .playlist(playlist)
        .expect("playlist view")
}

// ---------------------------------------------------------------------------
// Task 6.8 — repository integration gates：正常/空/错误/只读/不可用状态
// ---------------------------------------------------------------------------

/// 空资料库：全部歌曲/喜欢/最近返回空页而非错误（`is_last = true`）。
#[test]
fn catalog_repository_gate_empty_library_returns_empty_pages() {
    let (_directory, database, _root) = database();
    let query = CatalogQuery::new(&database);

    let all = query
        .all_songs(SongSort::default(), None, 100)
        .expect("empty all songs");
    assert!(all.items.is_empty());
    assert!(all.is_last, "empty page is the last page");

    let favs = query
        .favorites(SongSort::default(), None, 100)
        .expect("empty favorites");
    assert!(favs.items.is_empty());

    let recent = query.recent_100().expect("empty recent");
    assert!(recent.is_empty());

    let p = PlaylistId::new();
    let empty = CatalogQuery::new(&database)
        .playlist(p)
        .expect("empty playlist");
    assert!(empty.is_empty(), "playlist queries tolerate unknown ids");
}

/// 错误状态：无 active 根时 catalog/playlist 查询返回不可用，而非空结果。
#[test]
fn catalog_repository_gate_no_active_root_is_unavailable() {
    let (_directory, database, _root) = database();
    // Remove active: no active root remains.
    // (The database() helper made one active; deactivate it.)
    // Deactivate requires the exact id — simpler: open a fresh DB with none.
    drop(database);
    let directory = tempfile::tempdir().expect("tempdir");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open");
    let query = CatalogQuery::new(&database);

    assert!(query.all_songs(SongSort::default(), None, 10).is_err());
    assert!(query.favorites(SongSort::default(), None, 10).is_err());
    assert!(query.recent_100().is_err());
}

/// 只读状态：write-safety-locked 根仍可读（catalog 查询可用），仅禁用写。
#[test]
fn catalog_repository_gate_read_only_root_still_serves_reads() {
    let (_directory, database, root) = database();
    // Lock writes via the safety isolation.
    LibraryRepository::set_write_safety_locked(&database, root, true).expect("lock");
    let song_rec = song(root, "r.flac", "R", "甲");
    SongRepository::upsert(&database, &song_rec).expect("song");

    let query = CatalogQuery::new(&database);
    let page = query
        .all_songs(SongSort::default(), None, 10)
        .expect("readonly root still serves reads");
    assert_eq!(page.items.len(), 1);
}

/// 错误路径：坏的 page limit 被拒绝；歌单查询对不存在歌单返回空。
#[test]
fn catalog_repository_gate_invalid_limit_rejected_playlist_unknown_empty() {
    let (_directory, database, root) = database();
    let song_rec = song(root, "e.flac", "E", "乙");
    SongRepository::upsert(&database, &song_rec).expect("song");
    let query = CatalogQuery::new(&database);

    assert!(query.all_songs(SongSort::default(), None, 0).is_err());
    assert!(query.all_songs(SongSort::default(), None, 501).is_err());
    let ghost = PlaylistId::new();
    assert!(query.playlist(ghost).expect("unknown playlist").is_empty());
}

/// 歌单仓库 gate：正常 CRUD、空成员、错误（重复名/未知 id）、只读与不可用
/// 根下成员关系与查询的行为。名字含 `playlists` 以满足 6.8 的
/// `cargo test ... playlists` 验收过滤。
#[test]
fn playlists_repository_gate_covers_normal_empty_error_and_root_states() {
    let (_directory, database, root) = database();
    let playlist = PlaylistId::new();
    database.create(playlist, root, "常规").expect("create");
    // Normal: create + rename + list.
    assert!(PlaylistRepository::by_id(&database, playlist)
        .expect("by_id")
        .is_some());
    database.rename(playlist, "常规二").expect("rename");
    assert_eq!(
        PlaylistRepository::list(&database, root)
            .expect("list")
            .len(),
        1
    );
    // Empty: fresh playlist has no members.
    assert!(PlaylistRepository::members(&database, playlist)
        .expect("members")
        .is_empty());
    // Error: duplicate name rejected; unknown id by_id is None.
    assert!(database.create(PlaylistId::new(), root, "常规二").is_err());
    assert!(PlaylistRepository::by_id(&database, PlaylistId::new())
        .expect("ghost")
        .is_none());

    // Read-only root still allows reads and membership lookup.
    LibraryRepository::set_write_safety_locked(&database, root, true).expect("lock");
    let song_rec = song(root, "s.flac", "S", "丙");
    SongRepository::upsert(&database, &song_rec).expect("song");
    database
        .add_member(playlist, song_rec.id(), u64::MAX)
        .expect("member");
    assert_eq!(
        PlaylistRepository::members(&database, playlist)
            .expect("readonly members")
            .len(),
        1,
        "readonly root keeps playlists readable"
    );
}

/// Task 12.5 benchmark: a 50,000-song synthetic library, asserting the PRD
/// latency budgets (search p95 ≤ 200 ms, view first-screen p95 ≤ 500 ms) over
/// the real SQLite query path. This is an acceptance *budget*, not a micro
/// bench — it runs a bounded number of iterations through `CatalogQuery` on a
/// temp DB and reports the p95. The budget is deliberately exclusive of scan /
/// fixture creation (only query latency matters for the UI feel).
///
/// On slow CI runners the absolute budgets may be flaky; but the PRD is a hard
/// p95 target, so the test fails loudly rather than being waived. (A synthetic
/// 50k seed on this machine is a few hundred ms.) Devs can run it with
/// `cargo test -p echo-core --all-features -- --ignored` if they only want the
/// other fast tests; it is `#[ignore]`d by default to keep normal `cargo test`
/// fast and deterministic, and the verify:task 12.5 check runs it explicitly.
#[test]
#[ignore = "run explicitly via the 12.5 benchmark check (seeds 50k into temp SQLite)"]
fn bench_50k_search_and_first_screen_p95_meet_prd_budgets() {
    use std::time::Instant;

    const N: u64 = 50_000;
    let (_directory, database, root) = database();
    // Seed exactly 50,000 songs. Every 5th carries the search token "合成" so
    // the trigram search is meaningful; titles/artists are varied for index
    // pressure.
    let now = Instant::now();
    for index in 0..N {
        let shared = index % 5 == 0;
        let title = if shared {
            format!("合成歌曲{index:05}")
        } else {
            format!("普通歌曲{index:05}")
        };
        let artist = format!("艺人{}", index % 97);
        let mut rec = song(root, &format!("audio/{index:05}.flac"), &title, &artist);
        rec.apply_metadata(
            Some(title.clone()),
            Some(artist),
            Some("合成专辑".to_owned()),
            Some(Duration::from_secs(200)),
        );
        SongRepository::upsert(&database, &rec).expect("seed");
    }
    let seed_secs = now.elapsed().as_secs_f64();

    let query = CatalogQuery::new(&database);
    let added = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Asc,
    };

    // --- Search p95 ≤ 200 ms ---
    let mut search_samples = Vec::new();
    for _ in 0..12 {
        let t = Instant::now();
        let page = query
            .search("合成歌曲", false, added, None, 50)
            .expect("search");
        search_samples.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!page.items.is_empty(), "search must hit the seeded token");
    }
    let search_p95 = p95(&mut search_samples);
    assert!(
        search_p95 <= 200.0,
        "search p95 {search_p95:.1} ms exceeded the 200 ms budget"
    );

    // --- First-screen (all_songs page 1) p95 ≤ 500 ms ---
    let mut view_samples = Vec::new();
    for _ in 0..12 {
        let t = Instant::now();
        let page = query.all_songs(added, None, 50).expect("first page");
        view_samples.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.items.len(), 50, "first screen = one viewport page");
    }
    let view_p95 = p95(&mut view_samples);
    assert!(
        view_p95 <= 500.0,
        "view first-screen p95 {view_p95:.1} ms exceeded the 500 ms budget"
    );

    let _ = std::io::Write::write_fmt(
        &mut std::io::stdout(),
        format_args!(
            "bench 50k: seeded {N} songs in {seed_secs:.1}s; search p95 {search_p95:.1} ms (≤200), first-screen p95 {view_p95:.1} ms (≤500)\n",
        ),
    );
}

// ---------------------------------------------------------------------------
// Task 12.x — residual infrastructure branches: journal deadlines/claims,
// root-state mutations, TxAccess isolation, scan-issue code mapping and the
// query limit gate.
// ---------------------------------------------------------------------------

#[test]
fn journal_deadlines_claims_and_item_state_round_trip() {
    let (_directory, database, root) = database();
    let song = song(root, "jd.flac", "JD", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    let operation = OperationId::new();
    database
        .create_operation(operation, root, "import", Some(song.id()))
        .expect("envelope");
    database
        .upsert_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::Planned,
                song: Some(song.id()),
                source: Some("hit".to_owned()),
                staging_path: Some(RelativeMediaPath::new("staged.bin").expect("path")),
                target_path: RelativeMediaPath::new("目标/曲.flac").expect("path"),
                expected_hash: "c".repeat(64),
                item_key: "audio".to_owned(),
                claim_key: "目标/曲.flac".to_owned(),
            },
        )
        .expect("item");

    // item_state is the per-item read the recovery journal drives.
    let item = database
        .item_state(operation, "audio")
        .expect("item state")
        .expect("present");
    assert_eq!(item.state, OperationState::Planned);
    assert_eq!(item.item_key, "audio");
    assert_eq!(item.source.as_deref(), Some("hit"));
    assert_eq!(item.claim_key, "目标/曲.flac");

    // Undo deadline set + read, both before and after the claim release.
    assert_eq!(database.undo_deadline(operation).expect("deadline"), None);
    database
        .set_undo_deadline(operation, 123_456_789)
        .expect("deadline");
    assert_eq!(
        database
            .undo_deadline(operation)
            .expect("deadline")
            .unwrap(),
        123_456_789
    );
    // The item_state read reports the exact state the recovery step wrote.
    database
        .upsert_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Audio,
                state: OperationState::CopyApplied,
                song: Some(song.id()),
                source: Some("hit".to_owned()),
                staging_path: Some(RelativeMediaPath::new("staged.bin").expect("path")),
                target_path: RelativeMediaPath::new("目标/曲.flac").expect("path"),
                expected_hash: "c".repeat(64),
                item_key: "audio".to_owned(),
                claim_key: "目标/曲.flac".to_owned(),
            },
        )
        .expect("state machine advance");
    assert_eq!(
        database
            .item_state(operation, "audio")
            .expect("item")
            .unwrap()
            .state,
        OperationState::CopyApplied
    );
    database.release_claims(operation).expect("release");
    assert_eq!(claim_active(&database, operation), 0);
    // A deadline remains observable after the claim release.
    assert_eq!(
        database
            .undo_deadline(operation)
            .expect("deadline")
            .unwrap(),
        123_456_789
    );
}

#[test]
fn root_state_mutations_flip_active_write_capability_and_safety_lock() {
    let (directory, database, root) = database();
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), false, true),
    )
    .expect("second inactive root");

    // Deactivate the active root (only the root record changes; the id still
    // resolves to an inactive record).
    LibraryRepository::deactivate(&database, root).expect("deactivate");
    let stored = LibraryRepository::by_id(&database, root)
        .expect("query")
        .expect("present");
    assert!(!stored.is_active(), "the root record is inactive now");
    assert!(
        database.active_root().expect("active query").is_none(),
        "no root is active once the only active one is deactivated"
    );

    // Re-activate the second root and flip its write/availability flags in
    // one transaction.
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), true, true),
    )
    .expect("reactivate second root");
    LibraryRepository::set_write_and_availability(&database, other, false, false).expect("flip");
    let flipped = LibraryRepository::by_id(&database, other)
        .expect("query")
        .expect("present");
    assert!(
        !flipped.observed_write_capable(),
        "write_capable flipped off"
    );
    assert!(
        flipped.availability() == RootAvailability::Unavailable,
        "availability flipped to unavailable"
    );

    // The safety lock can be toggled independently.
    LibraryRepository::set_write_safety_locked(&database, other, true).expect("lock on");
    LibraryRepository::set_write_safety_locked(&database, other, false).expect("lock off");
    assert!(!LibraryRepository::by_id(&database, other)
        .expect("query")
        .expect("present")
        .write_safety_locked());
}

#[test]
fn tx_access_isolates_root_writes_and_exposes_every_write_surface() {
    let (_directory, database, root) = database();
    let song = song(root, "tx.flac", "TX", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    // isolate_root_writes flips the safety lock and availability atomically.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.isolate_root_writes(root, false)
        }))
        .expect("isolate");
    let frozen = LibraryRepository::by_id(&database, root)
        .expect("query")
        .expect("present");
    assert!(frozen.write_safety_locked(), "writes isolated");
    assert!(frozen.availability() == RootAvailability::Unavailable);

    // The remaining write surface works inside a UnitOfWork transaction:
    // roots, playlists, members, journal claims, lyrics and runtime state.
    let other_root = LibraryRootId::new();
    let other = LibraryRoot::new(other_root, database.path().join("other"), false, true);
    let playlist = PlaylistId::new();
    let staging = RelativeMediaPath::new("trash/op-audio").expect("path");
    let target = RelativeMediaPath::new("目标/曲.flac").expect("path");
    let operation = OperationId::new();
    let song_id = song.id();
    // The journal envelope must exist before an item can attach to it.
    database
        .create_operation(operation, root, "import", None)
        .expect("envelope");
    let candidate = LyricsCandidate::new(
        crate::domain::entities::LyricsSource::Embedded,
        vec![LyricsLine {
            timestamp_ms: 0,
            text: "a".to_owned(),
            original_index: 0,
        }],
        false,
    );
    let result: Result<(), Error> = database.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        tx.upsert_root(&other)?;
        tx.create_playlist(playlist, root, "存在")?;
        tx.insert_member(&PlaylistMember::new(
            playlist,
            song.id(),
            0,
            SongAvailability::Available,
        ))?;
        tx.upsert_operation_item(
            operation,
            OperationItem {
                kind: OperationResourceKind::Lyrics,
                state: OperationState::Planned,
                song: Some(song_id),
                source: None,
                staging_path: Some(staging.clone()),
                target_path: target.clone(),
                expected_hash: "d".repeat(64),
                item_key: "lyrics".to_owned(),
                claim_key: target.display().to_owned(),
            },
        )?;
        tx.set_lyrics_candidate(song_id, &candidate)?;
        tx.clear_lyrics_candidate(song_id, crate::domain::entities::LyricsSource::Embedded)?;
        tx.set_runtime_state("test-key", "test-value")
    }));
    // Clear the just-written candidate in the same transaction proves the
    // clear path is atomic with the write it removes.
    result.expect("tx write surface");

    assert!(LibraryRepository::by_id(&database, other_root)
        .expect("query")
        .expect("present")
        .absolute_path()
        .ends_with("other"));
    assert_eq!(
        PlaylistRepository::by_id(&database, playlist).unwrap(),
        Some(playlist)
    );
    assert_eq!(database.members(playlist).expect("members").len(), 1);
    assert_eq!(
        database
            .item_state(operation, "lyrics")
            .expect("item")
            .unwrap()
            .kind,
        OperationResourceKind::Lyrics
    );
    assert_eq!(
        database.load("test-key").expect("runtime state").as_deref(),
        Some("test-value")
    );
    // The lyrics candidate was cleared by the same transaction.
    assert!(database.candidates(song_id).expect("candidates").is_empty());
}

#[test]
fn scan_issue_codes_map_deterministically_and_unknown_codes_are_scan_file_error() {
    let (_directory, database, root) = database();
    crate::application::ports::ScanRunRepository::begin_run(&database, root, 1)
        .expect("run row exists for the FK");
    for code in [
        "unsupported_media",
        "no_audio_track",
        "corrupt_media",
        "duplicate_content",
        "tag_limit",
        "unknown_thing",
    ] {
        let diagnostic = MediaDiagnostic::new(
            RelativeMediaPath::new(&format!("issues/{code}.mp3")).expect("path"),
            code,
            format!("detail {code}"),
            false,
        );
        database
            .record_issue(root, 1, &diagnostic)
            .expect("record issue");
    }

    let issues = database.scan_issues(root, 1).expect("issues");
    // record_issue stores the code unchanged; scan_issues maps unknown codes
    // to the stable `scan_file_error`.
    let mapped: Vec<(&str, &str)> = issues
        .iter()
        .map(|issue| (issue.code(), issue.path().display()))
        .collect();
    assert!(mapped.contains(&("unsupported_media", "issues/unsupported_media.mp3")));
    assert!(mapped.contains(&("no_audio_track", "issues/no_audio_track.mp3")));
    assert!(mapped.contains(&("corrupt_media", "issues/corrupt_media.mp3")));
    assert!(mapped.contains(&("duplicate_content", "issues/duplicate_content.mp3")));
    assert!(mapped.contains(&("tag_limit", "issues/tag_limit.mp3")));
    // The unknown stored code normalizes to the generic scan-file-error.
    assert!(
        mapped.iter().any(|(code, path)| {
            *code == "scan_file_error" && *path == "issues/unknown_thing.mp3"
        }),
        "unknown stored issue codes map to scan_file_error: {mapped:?}"
    );
}

#[test]
fn page_limit_gate_rejects_zero_and_oversized_limits() {
    let (_directory, database, _root) = database();
    let sort = SongSort::default();
    for limit in [0usize, 501] {
        let error = database
            .query_active_songs("", sort, None, limit)
            .expect_err("limit rejected");
        assert_eq!(error.code(), "validation", "limit {limit}");
        let error = database
            .search_active_songs("", sort, None, limit)
            .expect_err("limit rejected");
        assert_eq!(error.code(), "validation", "search limit {limit}");
    }
    // Exact boundary values are accepted (empty result, not an error).
    assert!(database
        .query_active_songs("", sort, None, 1)
        .expect("limit 1")
        .items
        .is_empty());
    assert!(database
        .query_active_songs("", sort, None, 500)
        .expect("limit 500")
        .items
        .is_empty());
}
