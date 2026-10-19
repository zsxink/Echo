use super::*;

fn connection() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().expect("connection");
    connection
        .execute_batch(
            "CREATE TABLE songs(uuid TEXT PRIMARY KEY, is_favorite INTEGER NOT NULL DEFAULT 0, favorited_at INTEGER, play_count INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE sync_song_aliases(alias_song_uuid TEXT PRIMARY KEY, canonical_song_uuid TEXT, commit_seq INTEGER);
             CREATE TABLE sync_mutations(object_type TEXT, object_uuid TEXT, status TEXT);
             CREATE TABLE sync_object_versions(object_type TEXT, object_uuid TEXT, payload_json TEXT, is_tombstone INTEGER);
             CREATE TABLE sync_local_delete_intents(object_type TEXT, object_uuid TEXT, state TEXT);
             CREATE TABLE sync_device_play_counts(song_uuid TEXT, device_id TEXT, play_count INTEGER, PRIMARY KEY(song_uuid, device_id));
             INSERT INTO songs(uuid) VALUES('song');",
        )
        .expect("schema");
    connection
}

#[test]
fn confirmed_favorite_projects_without_touching_pending_overlay() {
    let mut connection = connection();
    let tx = connection.transaction().expect("transaction");
    project_confirmed_song_state(
        &tx,
        "favorite",
        "song",
        &serde_json::json!({"song_uuid":"song","is_favorite":true,"favorited_at":42}),
        None,
    )
    .expect("projection");
    let state: (i64, Option<i64>) = tx
        .query_row("SELECT is_favorite, favorited_at FROM songs", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("favorite");
    assert_eq!(state, (1, Some(42)));
    tx.execute(
        "INSERT INTO sync_mutations(object_type, object_uuid, status) VALUES('favorite','song','pending')",
        [],
    )
    .expect("pending overlay");
    project_confirmed_song_state(
        &tx,
        "favorite",
        "song",
        &serde_json::json!({"song_uuid":"song","is_favorite":false,"favorited_at":null}),
        None,
    )
    .expect("confirmed base does not overwrite pending view");
    let favorite: i64 = tx
        .query_row("SELECT is_favorite FROM songs", [], |row| row.get(0))
        .expect("favorite");
    assert_eq!(favorite, 1);
}

#[test]
fn play_stats_merge_each_device_by_max_and_recompute_total() {
    let mut connection = connection();
    let tx = connection.transaction().expect("transaction");
    tx.execute(
        "INSERT INTO sync_device_play_counts VALUES('song','00000000-0000-0000-0000-000000000001',4)",
        [],
    )
    .expect("existing bucket");
    tx.execute(
        "INSERT INTO sync_mutations VALUES('play-stats','song','pending')",
        [],
    )
    .expect("pending local play");
    project_confirmed_song_state(
        &tx,
        "play-stats",
        "song",
        &serde_json::json!({"song_uuid":"song","by_device":{
            "00000000-0000-0000-0000-000000000001":2,
            "00000000-0000-0000-0000-000000000002":3
        }}),
        None,
    )
    .expect("projection");
    let total: i64 = tx
        .query_row("SELECT play_count FROM songs", [], |row| row.get(0))
        .expect("total");
    assert_eq!(total, 7);
}

#[test]
fn play_stats_total_saturates_at_sqlite_integer_limit() {
    let mut connection = connection();
    let tx = connection.transaction().expect("transaction");
    project_confirmed_song_state(
        &tx,
        "play-stats",
        "song",
        &serde_json::json!({"song_uuid":"song","by_device":{
            "00000000-0000-0000-0000-000000000001":18_446_744_073_709_551_615_u64,
            "00000000-0000-0000-0000-000000000002":1
        }}),
        None,
    )
    .expect("saturating projection");
    let total: i64 = tx
        .query_row("SELECT play_count FROM songs", [], |row| row.get(0))
        .expect("total");
    assert_eq!(total, i64::MAX);
}

#[test]
fn play_stats_before_song_merge_into_a_durable_complete_confirmed_base() {
    let mut connection = connection();
    let song = "00000000-0000-0000-0000-000000000010";
    connection
        .execute_batch(
            "CREATE TABLE device_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             INSERT INTO device_state(key,value) VALUES('device_id','00000000-0000-0000-0000-000000000003');",
        )
        .expect("local device schema");
    let tx = connection.transaction().expect("transaction");
    tx.execute("DELETE FROM songs WHERE uuid=?1", [song])
        .expect("song not yet received");
    project_confirmed_song_state(
        &tx,
        "play-stats",
        song,
        &serde_json::json!({"song_uuid":song,"by_device":{"00000000-0000-0000-0000-000000000001":2}}),
        None,
    )
    .expect("first device bucket");
    project_confirmed_song_state(
        &tx,
        "play-stats",
        song,
        &serde_json::json!({"song_uuid":song,"by_device":{"00000000-0000-0000-0000-000000000002":3}}),
        None,
    )
    .expect("second device bucket");
    let base = confirmed_play_stats_payload(&tx, song).expect("merged base");
    assert_eq!(base["by_device"]["00000000-0000-0000-0000-000000000001"], 2);
    assert_eq!(base["by_device"]["00000000-0000-0000-0000-000000000002"], 3);
}

fn song_projection_connection() -> Connection {
    let connection = rusqlite::Connection::open_in_memory().expect("connection");
    connection
        .execute_batch(
            "CREATE TABLE songs(
                uuid TEXT PRIMARY KEY, library_root_uuid TEXT NOT NULL,
                relative_path TEXT NOT NULL, normalized_relative_path TEXT NOT NULL,
                blake3_hash TEXT, file_size INTEGER, file_mtime_ns INTEGER,
                format TEXT, title TEXT, artist TEXT, album TEXT,
                title_sort TEXT, artist_sort TEXT, album_sort TEXT,
                duration_ms INTEGER, is_favorite INTEGER DEFAULT 0,
                favorited_at INTEGER, play_count INTEGER DEFAULT 0, added_at INTEGER,
                availability TEXT, revision INTEGER, bitrate_bps INTEGER,
                sample_rate_hz INTEGER, channels INTEGER, bits_per_sample INTEGER,
                created_at INTEGER, updated_at INTEGER,
                UNIQUE(library_root_uuid, normalized_relative_path));
             CREATE VIRTUAL TABLE song_search USING fts5(title, artist, album, song_uuid UNINDEXED);
             CREATE TABLE sync_song_aliases(alias_song_uuid TEXT PRIMARY KEY, canonical_song_uuid TEXT, commit_seq INTEGER);
             CREATE TABLE sync_mutations(object_type TEXT, object_uuid TEXT, status TEXT, local_root_uuid TEXT);
             CREATE TABLE sync_object_versions(object_type TEXT, object_uuid TEXT, payload_json TEXT, is_tombstone INTEGER);
             CREATE TABLE sync_local_delete_intents(object_type TEXT, object_uuid TEXT, state TEXT);
             INSERT INTO songs(uuid, library_root_uuid, relative_path, normalized_relative_path, title_sort, artist_sort, album_sort, added_at, availability, created_at, updated_at)
             VALUES('song','local-root','old.flac','old.flac','','','',1,'missing',1,1);",
        )
        .expect("schema");
    connection
}

fn song_projection_payload() -> Value {
    serde_json::json!({
        "song_uuid":"song", "relative_path":"Music/song.flac",
        "title":"Title", "artist":"Artist", "album":null,
        "blake3_hash":"a".repeat(64), "file_size":123,
        "duration_ms":456, "format":"flac", "bitrate_bps":900_000,
        "sample_rate_hz":48_000, "channels":2, "bits_per_sample":24,
        "added_at":777
    })
}

#[test]
fn confirmed_song_materializes_as_missing_and_does_not_create_local_outbox() {
    let mut connection = song_projection_connection();
    let tx = connection.transaction().expect("transaction");
    tx.execute(
        "INSERT INTO sync_object_versions VALUES('favorite','song',?1,0)",
        params![r#"{"song_uuid":"song","is_favorite":true,"favorited_at":77}"#],
    )
    .expect("favorite arrives before song");
    project_confirmed_song_state(
        &tx,
        "song",
        "song",
        &song_projection_payload(),
        Some("local-root"),
    )
    .expect("song projection");
    let materialized: (String, String, String, String, i64) = tx
        .query_row(
            "SELECT relative_path, title, artist, availability, added_at FROM songs WHERE uuid='song'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .expect("song");
    assert_eq!(
        materialized,
        (
            "Music/song.flac".into(),
            "Title".into(),
            "Artist".into(),
            "missing".into(),
            777
        )
    );
    let favorite: (i64, Option<i64>) = tx
        .query_row(
            "SELECT is_favorite, favorited_at FROM songs WHERE uuid='song'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("delayed favorite projection");
    assert_eq!(favorite, (1, Some(77)));
    let indexed: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM song_search WHERE song_uuid='song'",
            [],
            |row| row.get(0),
        )
        .expect("search index");
    assert_eq!(indexed, 1);
    tx.execute(
        "INSERT INTO sync_mutations VALUES('song','song','pending','local-root')",
        [],
    )
    .expect("pending local edit");
    let mut later_remote = song_projection_payload();
    // An unconfirmed local edit keeps its effective view over a new base.
    later_remote["title"] = Value::String("New confirmed base".into());
    project_confirmed_song_state(&tx, "song", "song", &later_remote, Some("local-root"))
        .expect("preserve pending song overlay");
    let title: String = tx
        .query_row("SELECT title FROM songs WHERE uuid='song'", [], |row| {
            row.get(0)
        })
        .expect("effective title");
    assert_eq!(title, "Title");
    tx.execute("DELETE FROM sync_mutations", [])
        .expect("resolve pending edit");
    tx.execute(
        "INSERT INTO sync_local_delete_intents VALUES('song','song','open')",
        [],
    )
    .expect("open undo intent");
    later_remote["relative_path"] = Value::String("Music/renamed.flac".into());
    later_remote["blake3_hash"] = Value::String("b".repeat(64));
    project_confirmed_song_state(&tx, "song", "song", &later_remote, Some("local-root"))
        .expect("undo overlay protects resource identity");
    let path: String = tx
        .query_row(
            "SELECT relative_path FROM songs WHERE uuid='song'",
            [],
            |row| row.get(0),
        )
        .expect("preserved path");
    assert_eq!(path, "Music/song.flac");
    tx.execute(
        "UPDATE sync_local_delete_intents SET state='cancelled' WHERE object_type='song' AND object_uuid='song'",
        [],
    )
    .expect("undo intent cancelled");
    project_confirmed_song_state(&tx, "song", "song", &later_remote, Some("local-root"))
        .expect("replay confirmed base after undo");
    let (path, availability): (String, String) = tx
        .query_row(
            "SELECT relative_path, availability FROM songs WHERE uuid='song'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("replayed base");
    assert_eq!(path, "Music/renamed.flac");
    assert_eq!(availability, "missing");
}

#[test]
fn playlist_projection_keeps_stable_member_identity_and_pending_local_view() {
    let mut connection = rusqlite::Connection::open_in_memory().expect("connection");
    connection
        .execute_batch(
            "CREATE TABLE sync_song_aliases(alias_song_uuid TEXT PRIMARY KEY, canonical_song_uuid TEXT, commit_seq INTEGER);
             CREATE TABLE sync_mutations(object_type TEXT, object_uuid TEXT, status TEXT);
             CREATE TABLE sync_object_versions(object_type TEXT, object_uuid TEXT, payload_json TEXT, is_tombstone INTEGER);
             CREATE TABLE songs(uuid TEXT PRIMARY KEY, library_root_uuid TEXT);
             CREATE TABLE playlists(uuid TEXT PRIMARY KEY, library_root_uuid TEXT, display_name TEXT, normalized_name_key TEXT, created_at INTEGER, updated_at INTEGER, UNIQUE(library_root_uuid, normalized_name_key));
             CREATE TABLE playlist_songs(playlist_uuid TEXT, song_uuid TEXT, position INTEGER, added_at INTEGER, member_uuid TEXT, PRIMARY KEY(playlist_uuid, song_uuid), UNIQUE(playlist_uuid, position));
             INSERT INTO songs VALUES('00000000-0000-0000-0000-000000000002','local-root');",
        )
        .expect("schema");
    let tx = connection.transaction().expect("transaction");
    let payload = serde_json::json!({
        "playlist_uuid":"playlist", "display_name":"Favorites", "created_at":100,
        "members":[{"song_uuid":"00000000-0000-0000-0000-000000000002","member_uuid":"00000000-0000-0000-0000-000000000001","position":0,"added_at":101}]
    });
    project_confirmed_song_state(&tx, "playlist", "playlist", &payload, Some("local-root"))
        .expect("playlist projection");
    let member: (String, i64) = tx
        .query_row(
            "SELECT member_uuid, position FROM playlist_songs WHERE playlist_uuid='playlist'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("member");
    assert_eq!(member, ("00000000-0000-0000-0000-000000000001".into(), 0));
    tx.execute(
        "INSERT INTO sync_mutations VALUES('playlist','playlist','pending')",
        [],
    )
    .expect("pending local playlist edit");
    let changed = serde_json::json!({
        "playlist_uuid":"playlist", "display_name":"Remote name", "created_at":100,
        "members":[]
    });
    project_confirmed_song_state(&tx, "playlist", "playlist", &changed, Some("local-root"))
        .expect("pending view stays effective");
    let name: String = tx
        .query_row(
            "SELECT display_name FROM playlists WHERE uuid='playlist'",
            [],
            |row| row.get(0),
        )
        .expect("name");
    let members: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM playlist_songs WHERE playlist_uuid='playlist'",
            [],
            |row| row.get(0),
        )
        .expect("members");
    assert_eq!(name, "Favorites");
    assert_eq!(members, 1);
}
