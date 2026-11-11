//! Migration 0031 / loudness projection compatibility tests.

use super::*;
use crate::domain::media::{LoudnessSource, LoudnessTags};
use crate::infrastructure::sqlite::connection::{apply_migrations, open_writer};

const LOUDNESS_COLUMNS: [&str; 5] = [
    "loudness_source",
    "loudness_track_gain_mdb",
    "loudness_album_gain_mdb",
    "loudness_track_peak_ppm",
    "loudness_album_peak_ppm",
];

fn tagged(source: LoudnessSource, gain_mdb: i32) -> LoudnessTags {
    LoudnessTags::new(source, Some(gain_mdb), None, Some(900_000), None)
}

#[test]
fn migration_0031_adds_nullable_loudness_columns() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let path = directory.path().join("columns.db");
    let mut connection = open_writer(&path).expect("writer");
    apply_migrations(&mut connection).expect("migrations");

    let mut statement = connection
        .prepare("PRAGMA table_info(songs)")
        .expect("table info");
    let columns: Vec<(String, i64)> = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    for name in LOUDNESS_COLUMNS {
        let column = columns
            .iter()
            .find(|(column, _)| column == name)
            .unwrap_or_else(|| panic!("missing column {name}"));
        assert_eq!(column.1, 0, "{name} must be nullable for old rows");
    }
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM schema_migrations WHERE version=31",
                [],
                |row| row.get(0)
            )
            .expect("version row"),
        1,
        "migration 31 is registered in order"
    );
}

#[test]
fn loudness_tags_round_trip_and_old_rows_report_no_tags() {
    let (_directory, database, root) = database();
    let mut song = song(root, "loud.flac", "Loud", "artist");
    SongRepository::upsert(&database, &song).expect("initial song");
    assert_eq!(
        SongRepository::by_id(&database, song.id())
            .expect("read")
            .expect("present")
            .loudness_tags(),
        None,
        "a row written without tags stays untagged"
    );

    song.apply_loudness(Some(tagged(LoudnessSource::ReplayGain2, -6_540)));
    SongRepository::upsert(&database, &song).expect("upsert tags");
    let stored = SongRepository::by_id(&database, song.id())
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.loudness_tags(),
        Some(tagged(LoudnessSource::ReplayGain2, -6_540))
    );
    // A read never re-parses the file: without a new upsert the value stays.
    assert_eq!(
        SongRepository::by_id(&database, song.id())
            .expect("read again")
            .expect("present")
            .loudness_tags(),
        stored.loudness_tags()
    );
}

#[test]
fn rescan_of_externally_changed_tags_replaces_the_gain() {
    let (_directory, database, root) = database();
    let mut song = song(root, "r128.opus", "R128", "artist");
    song.apply_loudness(Some(tagged(LoudnessSource::R128, -5_500)));
    SongRepository::upsert(&database, &song).expect("initial tags");

    song.apply_loudness(None);
    SongRepository::upsert(&database, &song).expect("rescan without tags");
    assert_eq!(
        SongRepository::by_id(&database, song.id())
            .expect("read")
            .expect("present")
            .loudness_tags(),
        None,
        "a rescan that finds no tags must clear a stale gain"
    );

    song.apply_loudness(Some(tagged(LoudnessSource::ReplayGain2, -9_000)));
    SongRepository::upsert(&database, &song).expect("rescan with new tags");
    let stored = SongRepository::by_id(&database, song.id())
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.loudness_tags(),
        Some(tagged(LoudnessSource::ReplayGain2, -9_000))
    );
    assert_eq!(
        stored.title(),
        Some("R128"),
        "user-facing metadata survives"
    );
    assert_eq!(stored.play_count().as_u64(), 0);
}

#[test]
fn sync_snapshot_payload_does_not_carry_loudness_columns() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let path = directory.path().join("payload.db");
    let database = SqliteDatabase::open(&path).expect("database");
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .expect("active root");
    let mut song = song(root, "sync.flac", "Sync", "artist");
    song.apply_loudness(Some(tagged(LoudnessSource::ReplayGain2, -3_000)));
    SongRepository::upsert(&database, &song).expect("upsert");

    let connection = open_writer(&path).expect("raw connection");
    let payload = crate::infrastructure::sqlite::statements::song_full_snapshot_payload(
        &connection,
        song.id(),
    )
    .expect("snapshot payload");
    let object = payload.as_object().expect("payload is an object");
    for name in LOUDNESS_COLUMNS {
        assert!(
            !object.contains_key(name),
            "{name} must not enter the sync payload"
        );
    }
    assert_eq!(object.get("title").and_then(|v| v.as_str()), Some("Sync"));
}
