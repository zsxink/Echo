//! Per-entity SQL statement functions: root/song/playlist/journal write paths.
//!
//! Called both from the direct repository impls and through [`super::SqliteTx`],
//! so each helper stays transaction-agnostic (the caller owns atomicity).

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use rusqlite::params;
use rusqlite::{Connection, OptionalExtension};

use crate::application::ports::{OperationItem, OperationResourceKind};
use crate::domain::entities::{LibraryRoot, RootAvailability, Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use crate::error::Error;

use super::conversion::{availability_to_db, operation_state_to_db};
use super::support::{map_constraint, now_ms, storage};
use crate::domain::text::{normalized_key, playlist_name_key};

#[allow(clippy::too_many_lines)]
pub(crate) fn upsert_root(connection: &Connection, root: &LibraryRoot) -> Result<(), Error> {
    let path = root.absolute_path().to_string_lossy().to_string();
    let key = normalized_key(&path);
    let now = now_ms();
    // Calls are serialized by the writer actor; when invoked through a
    // UnitOfWork the surrounding transaction supplies atomicity. Avoid opening
    // a nested SQLite transaction here so the same helper is valid in both.
    if root.is_active() {
        connection
            .execute(
                "UPDATE library_roots SET is_active = 0 WHERE is_active = 1 AND uuid <> ?1",
                params![root.id().to_string()],
            )
            .map_err(storage)?;
    }
    connection.execute("INSERT INTO library_roots (uuid, absolute_path, normalized_path_key, is_active, write_capable, availability, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) ON CONFLICT(uuid) DO UPDATE SET absolute_path = excluded.absolute_path, normalized_path_key = excluded.normalized_path_key, is_active = excluded.is_active, write_capable = excluded.write_capable, availability = excluded.availability, updated_at = excluded.updated_at", params![root.id().to_string(), path, key, i64::from(root.is_active()), i64::from(root.write_capable()), if root.availability() == RootAvailability::Available { "available" } else { "unavailable" }, now]).map_err(map_constraint)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn upsert_song(connection: &Connection, song: &Song) -> Result<(), Error> {
    let now = now_ms();
    let title = song.title().map(ToOwned::to_owned);
    let artist = song.artist().map(ToOwned::to_owned);
    let album = song.album().map(ToOwned::to_owned);
    // The upsert carries *parsed metadata*, never user state: a scan (or any
    // caller) may hold a stale snapshot, so `is_favorite`, `play_count` and
    // `availability` are intentionally absent from DO UPDATE. Those columns can
    // only change through their dedicated mutations, and a concurrent scan
    // writing back must never roll a favorite or play count backward.
    connection.execute("INSERT INTO songs (uuid, library_root_uuid, relative_path, normalized_relative_path, title, artist, album, title_sort, artist_sort, album_sort, duration_ms, is_favorite, play_count, added_at, availability, revision, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17) ON CONFLICT(uuid) DO UPDATE SET library_root_uuid = excluded.library_root_uuid, relative_path = excluded.relative_path, normalized_relative_path = excluded.normalized_relative_path, title = excluded.title, artist = excluded.artist, album = excluded.album, title_sort = excluded.title_sort, artist_sort = excluded.artist_sort, album_sort = excluded.album_sort, duration_ms = excluded.duration_ms, revision = excluded.revision, updated_at = excluded.updated_at", params![song.id().to_string(), song.root().to_string(), song.path().display(), song.path().identity_key(), title, artist, album, normalized_key(song.title().unwrap_or("")), normalized_key(song.artist().unwrap_or("")), normalized_key(song.album().unwrap_or("")), song.duration().map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)), i64::from(song.favorite()), i64::try_from(song.play_count().as_u64()).unwrap_or(i64::MAX), i64::try_from(song.added_at()).unwrap_or(i64::MAX), availability_to_db(song.availability()), i64::try_from(song.revision().as_u64()).unwrap_or(i64::MAX), now]).map_err(map_constraint)?;
    maintain_search(connection, song)?;
    touch_root(connection, song.root())
}

fn maintain_search(connection: &Connection, song: &Song) -> Result<(), Error> {
    connection
        .execute(
            "DELETE FROM song_search WHERE song_uuid = ?1",
            params![song.id().to_string()],
        )
        .map_err(storage)?;
    connection
        .execute(
            "INSERT INTO song_search (title, artist, album, song_uuid) VALUES (?1, ?2, ?3, ?4)",
            params![
                normalized_key(song.title().unwrap_or("")),
                normalized_key(song.artist().unwrap_or("")),
                normalized_key(song.album().unwrap_or("")),
                song.id().to_string()
            ],
        )
        .map_err(storage)?;
    Ok(())
}

pub(crate) fn set_song_availability(
    connection: &Connection,
    id: SongId,
    availability: SongAvailability,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE songs SET availability = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![id.to_string(), availability_to_db(availability), now_ms()],
        )
        .map_err(storage)?;
    touch_root_for_song(connection, id)
}
pub(crate) fn set_song_favorite(
    connection: &Connection,
    id: SongId,
    favorite: bool,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE songs SET is_favorite = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![id.to_string(), i64::from(favorite), now_ms()],
        )
        .map_err(storage)?;
    touch_root_for_song(connection, id)
}
pub(crate) fn increment_play_count(connection: &Connection, id: SongId) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE songs SET play_count = play_count + 1, updated_at = ?2 WHERE uuid = ?1",
            params![id.to_string(), now_ms()],
        )
        .map_err(storage)?;
    touch_root_for_song(connection, id)
}
fn touch_root(connection: &Connection, root: LibraryRootId) -> Result<(), Error> {
    connection.execute("UPDATE library_roots SET updated_at = CASE WHEN updated_at >= ?2 THEN updated_at + 1 ELSE ?2 END WHERE uuid = ?1", params![root.to_string(), now_ms()]).map_err(storage)?;
    Ok(())
}
pub(crate) fn touch_root_for_song(connection: &Connection, song: SongId) -> Result<(), Error> {
    connection.execute("UPDATE library_roots SET updated_at = CASE WHEN updated_at >= ?2 THEN updated_at + 1 ELSE ?2 END WHERE uuid = (SELECT library_root_uuid FROM songs WHERE uuid = ?1)", params![song.to_string(), now_ms()]).map_err(storage)?;
    Ok(())
}

pub(crate) fn create_playlist(
    connection: &Connection,
    id: PlaylistId,
    root: LibraryRootId,
    name: &str,
) -> Result<(), Error> {
    let now = now_ms();
    connection.execute("INSERT INTO playlists (uuid, library_root_uuid, display_name, normalized_name_key, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)", params![id.to_string(), root.to_string(), name, playlist_name_key(name), now]).map_err(map_constraint)?;
    Ok(())
}
pub(crate) fn add_member(
    connection: &Connection,
    playlist: PlaylistId,
    song: SongId,
    position: u64,
) -> Result<(), Error> {
    // Appending (u64::MAX) takes the next free position. Re-adding a song that
    // is already a member is idempotent (same position, no duplicate row). A
    // *position* clash with a different member is a real conflict and must
    // surface as an error — never silently swallowed (no INSERT OR IGNORE).
    let position = if position == u64::MAX {
        connection.query_row("SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_songs WHERE playlist_uuid = ?1", params![playlist.to_string()], |row| row.get::<_, u64>(0)).map_err(storage)?
    } else {
        position
    };
    let existing: Option<i64> = connection
        .query_row(
            "SELECT position FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
            params![playlist.to_string(), song.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if existing.is_some() {
        return Ok(());
    }
    connection.execute("INSERT INTO playlist_songs (playlist_uuid, song_uuid, position, added_at) VALUES (?1, ?2, ?3, ?4)", params![playlist.to_string(), song.to_string(), i64::try_from(position).unwrap_or(i64::MAX), now_ms()]).map_err(map_constraint)?;
    Ok(())
}

pub(crate) fn operation_item(
    connection: &Connection,
    operation: OperationId,
    item: &str,
) -> Result<Option<OperationItem>, Error> {
    connection.query_row("SELECT kind, state, song_uuid, target_relative_path, expected_hash, normalized_target_path FROM operation_items WHERE operation_uuid = ?1 AND item_key = ?2", params![operation.to_string(), item], super::conversion::operation_item_from_row).optional().map_err(storage)
}
pub(crate) fn upsert_operation_item(
    connection: &Connection,
    operation: OperationId,
    item: OperationItem,
) -> Result<(), Error> {
    let root: String = connection
        .query_row(
            "SELECT library_root_uuid FROM operation_journal WHERE operation_uuid = ?1",
            params![operation.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| Error::InvariantViolation {
            why: "operation item requires a persisted journal envelope".to_owned(),
        })?;
    connection.execute("INSERT INTO operation_items (operation_uuid, item_key, library_root_uuid, kind, state, song_uuid, target_relative_path, normalized_target_path, expected_hash, claim_active) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1) ON CONFLICT(operation_uuid, item_key) DO UPDATE SET state = excluded.state, song_uuid = excluded.song_uuid, target_relative_path = excluded.target_relative_path, normalized_target_path = excluded.normalized_target_path, expected_hash = excluded.expected_hash", params![operation.to_string(), item.claim_key, root, if item.kind == OperationResourceKind::Audio { "audio" } else { "lyrics" }, operation_state_to_db(item.state), item.song.map(|id| id.to_string()), item.target_path.display(), item.target_path.identity_key(), item.expected_hash]).map_err(map_constraint)?;
    Ok(())
}

/// Release every active target claim of an operation. Called when the
/// operation reaches a terminal state (completed, rolled back, delete
/// finalized or explicitly abandoned): until then the conditional unique index
/// keeps the target path reserved for the same reserved `SongId`.
pub(crate) fn release_operation_claims(
    connection: &Connection,
    operation: OperationId,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE operation_items SET claim_active = 0 WHERE operation_uuid = ?1 AND claim_active = 1",
            params![operation.to_string()],
        )
        .map_err(storage)?;
    Ok(())
}
