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
