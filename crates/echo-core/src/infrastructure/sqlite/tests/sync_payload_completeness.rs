use super::super::SqliteDatabase;
use crate::application::ports::{LibraryRepository, PlaylistRepository, SongRepository};
use crate::domain::entities::{LibraryRoot, Song, SongAvailability};
use crate::domain::ids::{
    LibraryRootId, PlaybackSessionId, PlaylistId, RelativeMediaPath, Revision, SongId,
};
use crate::domain::media::{AudioFormat, AudioParameters};
use serde_json::Value;
use std::time::Duration;

fn database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
    let directory = tempfile::tempdir().expect("directory");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("database");
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("local-only"), true, true),
    )
    .expect("root");
    // Shared payloads only exist for an enrolled root (SYN-R06-S04).
    database
        .bind_sync_library(
            crate::domain::sync::SyncBinding {
                library_id: crate::domain::sync::LibraryId::new(),
                authority_id: crate::domain::sync::AuthorityId::new(),
                epoch: crate::domain::sync::Epoch::INITIAL,
            },
            true,
            root,
        )
        .expect("bind sync library");
    (directory, database, root)
}

fn song(root: LibraryRootId, path: &str) -> Song {
    let mut song = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new(path).expect("path"),
        Revision::INITIAL,
    );
    song.apply_metadata(
        Some("标签".into()),
        Some("艺术家".into()),
        Some("专辑".into()),
        Some(Duration::from_millis(123_456)),
    );
    song
}

fn latest_payload(database: &SqliteDatabase, kind: &str, uuid: &str) -> Value {
    let kind = kind.to_owned();
    let uuid = uuid.to_owned();
    let json: String = database.with_reader(move |connection| {
        connection.query_row(
            "SELECT payload_json FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2 ORDER BY revision DESC LIMIT 1",
            rusqlite::params![kind, uuid], |row| row.get(0),
        ).map_err(super::super::support::storage)
    }).expect("outbox payload");
    serde_json::from_str(&json).expect("JSON")
}

#[test]
fn song_payload_preserves_media_identity_without_local_or_independent_state() {
    let (_directory, database, root) = database();
    let mut song = song(root, "音乐/标签.flac");
    song.apply_scan_facts_with_params(
        "a".repeat(64),
        456_789,
        123_000_000,
        AudioFormat::Flac,
        AudioParameters {
            bitrate_bps: Some(900_000),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            bits_per_sample: Some(24),
        },
    );
    SongRepository::upsert(&database, &song).expect("song");
    SongRepository::set_favorite(&database, song.id(), true).expect("favorite");
    let control = super::super::tests::playback_fixture::directory(&database, root);
    assert!(super::super::tests::playback_fixture::record(
        &database,
        &control,
        PlaybackSessionId::new(),
        song.id()
    ));
    SongRepository::set_availability(&database, song.id(), SongAvailability::Missing)
        .expect("missing");
    song.apply_metadata(Some("新标签".into()), None, None, None);
    SongRepository::upsert(&database, &song).expect("stale scan snapshot");
    let payload = latest_payload(&database, "song", &song.id().to_string());
    assert_eq!(payload["title"], "新标签");
    assert_eq!(payload["relative_path"], "音乐/标签.flac");
    assert_eq!(payload["blake3_hash"], "a".repeat(64));
    assert_eq!(payload["file_size"], 456_789);
    assert_eq!(payload["duration_ms"], 123_456);
    assert_eq!(payload["format"], "flac");
    assert_eq!(payload["bitrate_bps"], 900_000);
    assert_eq!(payload["sample_rate_hz"], 48_000);
    assert_eq!(payload["channels"], 2);
    assert_eq!(payload["bits_per_sample"], 24);
    assert_eq!(payload["added_at"], song.added_at());
    for forbidden in [
        "is_favorite",
        "favorited_at",
        "play_count",
        "availability",
        "file_mtime_ns",
        "absolute_path",
        "library_root_uuid",
        "normalized_relative_path",
    ] {
        assert!(
            payload.get(forbidden).is_none(),
            "shared song leaks {forbidden}"
        );
    }
    let favorite = latest_payload(&database, "favorite", &song.id().to_string());
    assert_eq!(favorite["is_favorite"], true);
    assert!(favorite["favorited_at"].as_i64().is_some());
    assert!(
        super::super::tests::playback_fixture::mutations(&control, root)
            .iter()
            .any(|candidate| candidate.object
                == crate::domain::sync::ObjectKey::PlayStats(song.id())
                && candidate.payload["by_device"].is_object())
    );
    let stored = SongRepository::by_id(&database, song.id())
        .expect("query")
        .expect("song");
    assert!(stored.favorite());
    assert_eq!(stored.play_count().as_u64(), 1);
    assert_eq!(stored.availability(), SongAvailability::Missing);
}

#[test]
fn every_playlist_mutation_keeps_a_complete_stable_shared_view() {
    let (_directory, database, root) = database();
    let first = song(root, "a.flac");
    let second = song(root, "b.flac");
    SongRepository::upsert(&database, &first).expect("first");
    SongRepository::upsert(&database, &second).expect("second");
    let playlist = PlaylistId::new();
    database.create(playlist, root, "原名").expect("create");
    let created = latest_payload(&database, "playlist", &playlist.to_string());
    assert_eq!(created["playlist_uuid"], playlist.to_string());
    assert_eq!(created["display_name"], "原名");
    assert_eq!(created["members"], serde_json::json!([]));
    assert!(created["created_at"].as_i64().is_some());
    database
        .add_member(playlist, first.id(), u64::MAX)
        .expect("append first");
    let one = latest_payload(&database, "playlist", &playlist.to_string());
    assert_eq!(one["display_name"], "原名");
    assert_eq!(one["members"][0]["song_uuid"], first.id().to_string());
    assert!(one["members"][0]["added_at"].as_i64().is_some());
    database
        .add_member(playlist, second.id(), u64::MAX)
        .expect("append second");
    let two = latest_payload(&database, "playlist", &playlist.to_string());
    assert_eq!(two["members"][0], one["members"][0]);
    assert_eq!(two["members"][1]["position"], 1);
    database.rename(playlist, "新名").expect("rename");
    let renamed = latest_payload(&database, "playlist", &playlist.to_string());
    assert_eq!(renamed["display_name"], "新名");
    assert_eq!(renamed["members"], two["members"]);
    assert_eq!(renamed["created_at"], created["created_at"]);
    database
        .remove_member(playlist, first.id())
        .expect("remove");
    let removed = latest_payload(&database, "playlist", &playlist.to_string());
    assert_eq!(removed["display_name"], "新名");
    assert_eq!(
        removed["members"],
        serde_json::json!([two["members"][1].clone()])
    );
    assert_eq!(removed["created_at"], created["created_at"]);
    assert!(removed.get("library_root_uuid").is_none());
    assert!(removed.get("cover_asset_key").is_none());
}
