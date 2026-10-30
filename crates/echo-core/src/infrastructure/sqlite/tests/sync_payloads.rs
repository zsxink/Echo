#[test]
fn playback_sessions_are_idempotent_and_writer_allows_concurrent_reads() {
    let (_directory, database, root) = sync_database();
    let song = song(root, "play.flac", "play", "artist");
    SongRepository::upsert(&database, &song).expect("song");
    let control = playback_fixture::directory(&database, root);
    let session = PlaybackSessionId::new();
    assert!(playback_fixture::record(&database, &control, session, song.id()));
    assert!(!playback_fixture::record(&database, &control, session, song.id()));
    assert!(playback_fixture::record(&database, &control, PlaybackSessionId::new(), song.id()));
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
        let control = control.clone();
        let song_id = song.id();
        std::thread::spawn(move || {
            for round in 0..40u64 {
                assert!(playback_fixture::record(&database, &control, PlaybackSessionId::new(), song_id));
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
    let buckets = database
        .device_play_counts(song.id())
        .expect("device buckets");
    assert_eq!(buckets.values().copied().sum::<u64>(), 42);
    let mutation_count = playback_fixture::mutations(&control, root).into_iter()
        .filter(|candidate| candidate.object == crate::domain::sync::ObjectKey::PlayStats(song.id()))
        .count();
    assert_eq!(
        mutation_count, 42,
        "each unique play is captured in its commit transaction"
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
        (1, include_str!("../migrations/0001_initial.sql")),
        (2, include_str!("../migrations/0002_library_assets.sql")),
        (3, include_str!("../migrations/0003_root_write_safety.sql")),
        (
            4,
            include_str!("../migrations/0004_song_audio_parameters.sql"),
        ),
        (5, include_str!("../migrations/0005_sync_foundation.sql")),
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

#[test]
fn migration_0010_adds_lan_ledger_idempotently_without_rewriting_prior_versions() {
    use crate::infrastructure::sqlite::connection::apply_migration_set;

    let directory = tempfile::tempdir().expect("temp dir");
    let mut direct = open_writer(&directory.path().join("ledger.db")).expect("connection");
    direct.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL);").expect("migration table");
    let prior: Vec<(i64, &str)> = vec![
        (1, include_str!("../migrations/0001_initial.sql")),
        (2, include_str!("../migrations/0002_library_assets.sql")),
        (3, include_str!("../migrations/0003_root_write_safety.sql")),
        (
            4,
            include_str!("../migrations/0004_song_audio_parameters.sql"),
        ),
        (5, include_str!("../migrations/0005_sync_foundation.sql")),
        (6, include_str!("../migrations/0006_portable_layout.sql")),
        (7, include_str!("../migrations/0007_playlist_covers.sql")),
        (8, include_str!("../migrations/0008_favorite_order.sql")),
        (9, include_str!("../migrations/0009_artist_covers.sql")),
    ];
    apply_migration_set(&mut direct, &prior).expect("legacy schema through 0009");
    direct.execute_batch(
        "INSERT INTO library_roots (uuid, absolute_path, normalized_path_key, is_active, write_capable, availability, created_at, updated_at)
             VALUES ('legacy-root', '/legacy', '/legacy', 1, 1, 'available', 1, 1);
         INSERT INTO songs (uuid, library_root_uuid, relative_path, normalized_relative_path, title, title_sort, artist_sort, album_sort, added_at, availability, created_at, updated_at)
             VALUES ('legacy-song', 'legacy-root', 'media/song.flac', 'media/song.flac', 'Song', 'song', 'artist', '', 7, 'available', 1, 1);",
    ).expect("seed legacy library");
    direct.execute(
        "INSERT INTO sync_outbox (object_type, object_uuid, revision, operation, payload_json, created_at) VALUES ('song', 'legacy-song', 5, 'upsert', '{}', 2)",
        [],
    ).expect("seed pre-ledger revision");
    apply_migrations(&mut direct).expect("apply ledger migration");
    apply_migrations(&mut direct).expect("reapplying migrations is a no-op");

    for table in [
        "sync_binding",
        "sync_peers",
        "sync_local_highwater",
        "sync_origin_cursors",
        "sync_device_play_counts",
        "sync_mutations",
        "sync_inbox",
        "sync_commits",
        "sync_object_versions",
        "sync_song_aliases",
        "sync_member_aliases",
        "sync_conflicts",
        "sync_peer_cursors",
        "sync_local_delete_intents",
        "sync_transfers",
        "sync_resources",
        "sync_resource_refs",
    ] {
        let count: i64 = direct
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .expect("table query");
        assert_eq!(count, 1, "missing {table}");
    }
    let prior_count: i64 = direct
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version <= 9",
            [],
            |row| row.get(0),
        )
        .expect("prior versions");
    assert_eq!(prior_count, 9);
    let ledger_count: i64 = direct
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 10",
            [],
            |row| row.get(0),
        )
        .expect("ledger version");
    assert_eq!(ledger_count, 1);
    let preserved_song: String = direct
        .query_row(
            "SELECT uuid FROM songs WHERE uuid = 'legacy-song'",
            [],
            |row| row.get(0),
        )
        .expect("legacy UUID preserved");
    assert_eq!(preserved_song, "legacy-song");
    let next_revision =
        super::sync::enqueue_sync(&direct, "song", "legacy-song", "{}", "upsert").unwrap();
    assert_eq!(
        next_revision, 6,
        "migration seeds the persistent high-water from legacy outbox"
    );
}

#[test]
fn migration_0011_separates_pending_local_sequence_from_host_confirmed_cursor() {
    use crate::infrastructure::sqlite::connection::apply_migration_set;

    let directory = tempfile::tempdir().expect("temp dir");
    let mut direct = open_writer(&directory.path().join("cursor-upgrade.db")).expect("connection");
    direct.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL);").expect("migration table");
    let through_ten: Vec<(i64, &str)> = vec![
        (1, include_str!("../migrations/0001_initial.sql")),
        (2, include_str!("../migrations/0002_library_assets.sql")),
        (3, include_str!("../migrations/0003_root_write_safety.sql")),
        (
            4,
            include_str!("../migrations/0004_song_audio_parameters.sql"),
        ),
        (5, include_str!("../migrations/0005_sync_foundation.sql")),
        (6, include_str!("../migrations/0006_portable_layout.sql")),
        (7, include_str!("../migrations/0007_playlist_covers.sql")),
        (8, include_str!("../migrations/0008_favorite_order.sql")),
        (9, include_str!("../migrations/0009_artist_covers.sql")),
        (10, include_str!("../migrations/0010_lan_sync_ledger.sql")),
    ];
    apply_migration_set(&mut direct, &through_ten).expect("legacy ledger");
    let device: String = direct
        .query_row(
            "SELECT value FROM device_state WHERE key='device_id'",
            [],
            |row| row.get(0),
        )
        .expect("device");
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let third = uuid::Uuid::new_v4().to_string();
    direct.execute("INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, 3, ?2)", params![device, third]).expect("legacy shared cursor");
    for (seq, id) in [
        (1, first.as_str()),
        (2, second.as_str()),
        (3, third.as_str()),
    ] {
        direct.execute("INSERT INTO sync_mutations(mutation_id, origin_device_id, origin_seq, object_type, object_uuid, operation, local_revision, payload_json, payload_digest, status, created_at) VALUES(?1, ?2, ?3, 'song', 'legacy-song', 'upsert', ?3, '{}', 'd', 'pending', 1)", params![id, device, seq]).expect("local pending event");
    }
    direct.execute("INSERT INTO sync_commits(commit_seq, library_id, authority_id, epoch, origin_device_id, mutation_id, canonical_json, previous_digest, commit_digest, publication_state, created_at) VALUES(1, 'lib', 'auth', 1, ?1, ?2, '{\"mutation\":{\"origin_seq\":1}}', zeroblob(32), randomblob(32), 'published', 1)", params![device, first]).expect("accepted event");

    apply_migrations(&mut direct).expect("apply cursor migration");
    let cursors: (i64, String, i64, String) = direct.query_row(
        "SELECT (SELECT origin_seq FROM sync_local_origin_cursors WHERE origin_device_id=?1), (SELECT last_mutation_id FROM sync_local_origin_cursors WHERE origin_device_id=?1), (SELECT origin_seq FROM sync_origin_cursors WHERE origin_device_id=?1), (SELECT last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1)",
        params![device], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).expect("separated cursors");
    assert_eq!(cursors, (3, third, 1, first));
}

#[path = "sync_payloads/lan_ledger.rs"]
mod lan_ledger;

/// Change-time arbitration and the host cursor a predecessor rejection reports.
#[path = "sync_payloads/change_time.rs"]
mod change_time;
use lan_ledger::{claim_item, seed_import_source};

#[path = "sync_payloads/deletion_and_stats.rs"]
mod deletion_and_stats;
