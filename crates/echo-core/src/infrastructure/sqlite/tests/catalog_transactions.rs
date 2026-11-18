//! `LL-R05-S03`: a catalog mutation that fails part-way must leave no trace.
//!
//! `PlaylistRepository::delete` and `remove_member` each write several facts —
//! an outbox mutation, a tombstone, the row change, and (for a member removal)
//! a playlist snapshot plus its HLC watermark. Without a transaction the
//! earlier statements commit one by one, so a failure in the last one leaves a
//! mutation queued for a row that still exists, or a row gone with no
//! tombstone to stop another device from resurrecting it. Each test fails the
//! *last* statement with a SQLite trigger and asserts all five artifacts are
//! back to their pre-operation state.

use super::*;
use crate::application::ports::{LibraryRepository, PlaylistRepository, SongRepository};
use crate::domain::entities::{LibraryRoot, Song};
use crate::domain::ids::{PlaylistId, RelativeMediaPath, Revision, SongId};
use crate::domain::sync::{AuthorityId, Epoch, LibraryId, SyncBinding};

/// Everything the two mutations touch, read in one consistent snapshot.
#[derive(Debug, Eq, PartialEq)]
struct LedgerState {
    /// The whole outbox: `(outbox_id, object_type, object_uuid, revision, operation, payload_json)`.
    outbox: Vec<(i64, String, String, i64, String, String)>,
    /// `(object_type, object_uuid, revision)`.
    tombstones: Vec<(String, String, i64)>,
    /// The per-object revision high-water mark.
    highwater: Vec<(String, String, i64)>,
    /// `(uuid, revision, hlc_wall_secs, hlc_counter)`.
    playlists: Vec<(String, i64, i64, i64)>,
    /// `(playlist_uuid, song_uuid, position, member_uuid)`.
    members: Vec<(String, String, i64, Option<String>)>,
}

fn ledger_state(database: &SqliteDatabase) -> LedgerState {
    database
        .with_reader(|connection| {
            let outbox = connection
                .prepare("SELECT outbox_id, object_type, object_uuid, revision, operation, payload_json FROM sync_outbox ORDER BY outbox_id")
                .map_err(storage)?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            let tombstones = connection
                .prepare("SELECT object_type, object_uuid, revision FROM tombstones ORDER BY object_type, object_uuid")
                .map_err(storage)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            let highwater = connection
                .prepare("SELECT object_type, object_uuid, local_revision FROM sync_local_highwater ORDER BY object_type, object_uuid")
                .map_err(storage)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            let playlists = connection
                .prepare("SELECT uuid, revision, hlc_wall_secs, hlc_counter FROM playlists ORDER BY uuid")
                .map_err(storage)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            let members = connection
                .prepare("SELECT playlist_uuid, song_uuid, position, member_uuid FROM playlist_songs ORDER BY playlist_uuid, song_uuid")
                .map_err(storage)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(LedgerState {
                outbox,
                tombstones,
                highwater,
                playlists,
                members,
            })
        })
        .expect("read ledger state")
}

/// A sync-bound database with one song and one playlist holding that song.
/// Binding matters: a standalone library mints no outbox rows at all, so the
/// partial-commit hazard would not exist.
fn bound_playlist() -> (tempfile::TempDir, SqliteDatabase, PlaylistId, SongId) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = SqliteDatabase::open(directory.path().join("echo.db")).expect("open database");
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .expect("insert active root");
    database
        .bind_sync_library(
            SyncBinding {
                library_id: LibraryId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            true,
            root,
        )
        .expect("bind sync library");

    let song = SongId::new();
    SongRepository::upsert(
        &database,
        &Song::new(
            song,
            root,
            RelativeMediaPath::new("media/artist/song.flac").expect("media path"),
            Revision::INITIAL,
        ),
    )
    .expect("insert song");

    let playlist = PlaylistId::new();
    PlaylistRepository::create(&database, playlist, root, "rollback").expect("create playlist");
    PlaylistRepository::add_member(&database, playlist, song, u64::MAX).expect("add member");
    (directory, database, playlist, song)
}

/// `LL-R05-S03`: `delete` enqueues the playlist tombstone *before* it removes
/// the row. Without one transaction a failure on the row deletion leaves the
/// mutation queued for a playlist that still exists — every device would learn
/// about a deletion that never happened here.
#[test]
fn catalog_delete_rolls_back_every_statement() {
    let (_directory, database, playlist, _song) = bound_playlist();
    let before = ledger_state(&database);
    assert!(
        before
            .outbox
            .iter()
            .any(|(_, kind, uuid, _, _, _)| kind == "playlist" && *uuid == playlist.to_string()),
        "the fixture queues a playlist fact to roll back: {before:?}"
    );

    // Fail the last statement — the row deletion — after the enqueue and the
    // tombstone have already run.
    database
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_playlist_delete BEFORE DELETE ON playlists
                     BEGIN SELECT RAISE(ABORT, 'injected playlist delete failure'); END;",
                )
                .map_err(storage)
        })
        .expect("install failure trigger");

    let error = PlaylistRepository::delete(&database, playlist).expect_err("the delete fails");
    assert!(
        error
            .to_string()
            .contains("injected playlist delete failure"),
        "{error}"
    );

    assert_eq!(
        ledger_state(&database),
        before,
        "a failed delete must not queue a mutation, write a tombstone, advance the \
         high-water mark or touch the row"
    );
    assert!(
        PlaylistRepository::name(&database, playlist)
            .expect("read playlist")
            .is_some(),
        "the playlist row is still present"
    );
}

/// `LL-R05-S03`: `remove_member` enqueues the member tombstone and deletes the
/// row before it re-snapshots the playlist and advances its HLC. A failure on
/// that last write without one transaction would leave the member removed
/// locally with the playlist's snapshot still listing it — and the outbox
/// holding a half-applied membership change.
#[test]
fn catalog_remove_member_rolls_back_every_statement() {
    let (_directory, database, playlist, song) = bound_playlist();
    let before = ledger_state(&database);
    assert_eq!(before.members.len(), 1, "one member to remove: {before:?}");

    // `advance_object_hlc` is the final statement and the only `UPDATE` that
    // names `hlc_wall_secs`, so this trigger fires after every other write.
    database
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_playlist_hlc BEFORE UPDATE OF hlc_wall_secs ON playlists
                     BEGIN SELECT RAISE(ABORT, 'injected member removal failure'); END;",
                )
                .map_err(storage)
        })
        .expect("install failure trigger");

    let error = PlaylistRepository::remove_member(&database, playlist, song)
        .expect_err("the removal fails");
    assert!(
        error
            .to_string()
            .contains("injected member removal failure"),
        "{error}"
    );

    assert_eq!(
        ledger_state(&database),
        before,
        "a failed removal must not queue the member delete or the playlist snapshot, \
         write a tombstone, delete the row or advance the high-water mark"
    );
    let members = PlaylistRepository::members(&database, playlist).expect("read members");
    assert_eq!(members.len(), 1, "the membership row is still present");
    assert_eq!(members[0].song(), song);
}
