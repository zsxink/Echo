//! SQLite infrastructure adapter for Echo's local-library truth source.
//!
//! SQL stays in this module: application code only sees repository ports and
//! domain values. A dedicated writer actor serializes every mutation while a
//! bounded pool of read-only connections serves catalog queries from WAL.
//!
//! Submodules:
//!
//! - [`actor`] — the single-writer actor that serializes every mutation.
//! - [`connection`] — connection lifecycle, migrations, backup, integrity.
//! - [`support`] — shared error mapping / timestamp / ID parsing glue.
//! - [`conversion`] — row mappers and enum ↔ db-string conversions.
//! - [`statements`] — per-entity SQL statement functions.
//! - [`query`] — keyset-paginated catalog queries.

// The public adapter methods share the application ports' uniform error
// contract; repeating identical `# Errors` sections would obscure their
// storage semantics. SQL assembly also deliberately uses explicit variables.
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args
)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::params;
use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::application::ports::{
    LibraryRepository, OperationItem, OperationJournalRepository, PlaylistRepository,
    SongRepository, TxAccess, UnitOfWork,
};
use crate::domain::catalog::{OpaqueCursor, Paged, SongSort};
use crate::domain::entities::{LibraryRoot, PlaylistMember, Song, SongAvailability};
use crate::domain::ids::{
    LibraryRootId, OperationId, PlaybackSessionId, PlaylistId, RelativeMediaPath, SongId,
};
use crate::domain::text::{normalized_key, playlist_name_key};
use crate::error::{Error, Subject};

pub(crate) mod actor;
pub(crate) mod connection;
pub(crate) mod conversion;
pub(crate) mod query;
pub(crate) mod statements;
pub(crate) mod support;
#[cfg(test)]
mod tests;

use actor::SqliteWriteActor;
#[cfg(test)]
use connection::apply_migration_set;
use connection::{
    apply_migrations, backup_connection, backup_path, file_is_non_empty, open_reader, open_writer,
    quick_check_connection, reader_count,
};
use conversion::{availability_from_db, operation_item_from_row, root_from_row, song_from_row};
use query::{active_root_id, query_active, SONG_SELECT};
use statements::{
    add_member, create_playlist, increment_play_count, operation_item, set_song_availability,
    set_song_favorite, upsert_operation_item, upsert_root, upsert_song,
};
use support::{map_constraint, now_ms, parse_id, storage, to_sql_error};

/// Real SQLite implementation of the library repositories and unit of work.
///
/// `open` applies ordered migrations before the actor starts, then owns one
/// write connection plus three read-only WAL connections. The type exposes no
/// `rusqlite` connection to application callers.
pub struct SqliteDatabase {
    path: PathBuf,
    writer: SqliteWriteActor,
    readers: Mutex<Vec<Connection>>,
}

impl SqliteDatabase {
    /// Open (or create) a database, configure safety pragmas, apply migrations
    /// and verify its quick integrity check.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let had_database = path.exists() && file_is_non_empty(&path)?;
        let mut writer = open_writer(&path)?;
        if had_database {
            let backup = backup_path(&path);
            backup_connection(&writer, &backup)?;
        }
        apply_migrations(&mut writer)?;
        quick_check_connection(&writer)?;

        let mut readers = Vec::with_capacity(reader_count());
        for _ in 0..reader_count() {
            readers.push(open_reader(&path)?);
        }
        Ok(Self {
            path,
            writer: SqliteWriteActor::spawn(writer),
            readers: Mutex::new(readers),
        })
    }

    /// Path of the local database; it is for runtime ownership only and must
    /// never be serialized to UI/sync payloads.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create a consistent SQLite backup using the SQLite backup API.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), Error> {
        let destination = destination.as_ref().to_path_buf();
        self.with_reader(|connection| backup_connection(connection, &destination))
    }

    /// Execute SQLite's cheap integrity probe. Call this before accepting a
    /// database as healthy after startup/recovery.
    pub fn quick_check(&self) -> Result<(), Error> {
        self.with_reader(quick_check_connection)
    }

    /// A deterministic schema snapshot used by migration integration tests.
    pub fn schema_snapshot(&self) -> Result<Vec<(String, String)>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection
                .prepare("SELECT name, sql FROM sqlite_master WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .map_err(storage)?;
            let snapshot = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default())))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(snapshot)
        })
    }

    /// Create the durable envelope required before adding journal items. The
    /// application later advances item states through `OperationJournalRepository`.
    pub fn create_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        let kind = kind.to_owned();
        self.writer.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO operation_journal (operation_uuid, library_root_uuid, kind, state, reserved_song_uuid, created_at, updated_at) VALUES (?1, ?2, ?3, 'planned', ?4, ?5, ?5)",
                    params![operation.to_string(), root.to_string(), kind, reserved_song.map(|id| id.to_string()), now_ms()],
                )
                .map_err(map_constraint)?;
            Ok(())
        })
    }

    /// Idempotently record a qualified library playback session. The song
    /// count is incremented only when the session row is newly inserted.
    pub fn record_playback(&self, session: PlaybackSessionId, song: SongId) -> Result<bool, Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO recorded_play_sessions (playback_session_uuid, song_uuid, recorded_at) VALUES (?1, ?2, ?3)",
                    params![session.to_string(), song.to_string(), now_ms()],
                )
                .map_err(storage)?;
            if inserted == 1 {
                transaction
                    .execute("UPDATE songs SET play_count = play_count + 1, updated_at = ?2 WHERE uuid = ?1", params![song.to_string(), now_ms()])
                    .map_err(storage)?;
                statements::touch_root_for_song(&transaction, song)?;
            }
            transaction.commit().map_err(storage)?;
            Ok(inserted == 1)
        })
    }

    /// Search only the active root. Three or more Unicode scalar values use
    /// the trigram FTS table; shorter values use escaped normalized LIKE.
    pub fn search_active_songs(
        &self,
        query: &str,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs(query, sort, cursor, limit)
    }

    /// Return a cursor page for the active root. Cursor boundaries are real
    /// keyset predicates over the selected sort columns, never `OFFSET`.
    pub fn query_active_songs(
        &self,
        query: &str,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        if limit == 0 || limit > 500 {
            return Err(Error::validation(
                Subject::Query,
                "page limit",
                "must be 1 through 500",
            ));
        }
        let query = normalized_key(query);
        let cursor = cursor.cloned();
        self.with_reader(move |connection| {
            query_active(connection, &query, sort, cursor.as_ref(), limit)
        })
    }

    /// The active root's newest 100 available songs, using stable UUID ties.
    pub fn recent_songs(&self) -> Result<Vec<Song>, Error> {
        self.with_reader(|connection| {
            let root = active_root_id(connection)?.ok_or_else(|| Error::unavailable("library", "no active root"))?;
            let mut statement = connection
                .prepare(&format!("{} WHERE s.library_root_uuid = ?1 AND s.availability = 'available' ORDER BY s.added_at DESC, s.uuid DESC LIMIT 100", SONG_SELECT))
                .map_err(storage)?;
            let songs = statement
                .query_map(params![root.to_string()], song_from_row)
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(songs)
        })
    }

    fn with_reader<T>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let connection = {
            let mut readers = self.readers.lock().map_err(|_| Error::InvariantViolation {
                why: "SQLite reader pool poisoned".to_owned(),
            })?;
            if let Some(connection) = readers.pop() {
                connection
            } else {
                open_reader(&self.path)?
            }
        };
        let result = read(&connection);
        let mut readers = self.readers.lock().map_err(|_| Error::InvariantViolation {
            why: "SQLite reader pool poisoned".to_owned(),
        })?;
        if readers.len() < reader_count() {
            readers.push(connection);
        }
        result
    }
}

impl LibraryRepository for SqliteDatabase {
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error> {
        self.with_reader(|connection| {
            connection
                .query_row("SELECT uuid, absolute_path, is_active, write_capable, availability FROM library_roots WHERE is_active = 1", [], root_from_row)
                .optional()
                .map_err(storage)
        })
    }

    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row("SELECT uuid, absolute_path, is_active, write_capable, availability FROM library_roots WHERE uuid = ?1", params![id.to_string()], root_from_row)
                .optional()
                .map_err(storage)
        })
    }

    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error> {
        let root = root.clone();
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_root(&transaction, &root)?;
            transaction.commit().map_err(storage)
        })
    }

    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection
                .execute(
                    "UPDATE library_roots SET is_active = 0, updated_at = ?2 WHERE uuid = ?1",
                    params![id.to_string(), now_ms()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection.execute("UPDATE library_roots SET write_capable = ?2, availability = ?3, updated_at = ?4 WHERE uuid = ?1", params![id.to_string(), i64::from(write_capable), if available { "available" } else { "unavailable" }, now_ms()]).map_err(storage)?;
            Ok(())
        })
    }
}

impl SongRepository for SqliteDatabase {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    &format!("{} WHERE s.uuid = ?1", SONG_SELECT),
                    params![id.to_string()],
                    song_from_row,
                )
                .optional()
                .map_err(storage)
        })
    }

    fn by_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<Song>, Error> {
        let path = path.normalized().to_owned();
        self.with_reader(move |connection| {
            connection
                .query_row(
                    &format!(
                        "{} WHERE s.library_root_uuid = ?1 AND s.normalized_relative_path = ?2",
                        SONG_SELECT
                    ),
                    params![root.to_string(), path],
                    song_from_row,
                )
                .optional()
                .map_err(storage)
        })
    }

    fn upsert(&self, song: &Song) -> Result<(), Error> {
        let song = song.clone();
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_song(&transaction, &song)?;
            transaction.commit().map_err(storage)
        })
    }

    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error> {
        self.writer
            .run(move |connection| set_song_availability(connection, id, availability))
    }

    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error> {
        self.writer
            .run(move |connection| set_song_favorite(connection, id, favorite))
    }

    fn increment_play_count(&self, id: SongId) -> Result<(), Error> {
        self.writer
            .run(move |connection| increment_play_count(connection, id))
    }
}

impl PlaylistRepository for SqliteDatabase {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT uuid FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage)?
                .map(|value| parse_id(&value, "PlaylistId"))
                .transpose()
        })
    }

    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error> {
        let key = playlist_name_key(normalized_name);
        self.with_reader(move |connection| connection.query_row("SELECT uuid FROM playlists WHERE library_root_uuid = ?1 AND normalized_name_key = ?2", params![root.to_string(), key], |row| row.get::<_, String>(0)).optional().map_err(storage)?.map(|value| parse_id(&value, "PlaylistId")).transpose())
    }

    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT uuid FROM playlists WHERE library_root_uuid = ?1 ORDER BY normalized_name_key, uuid").map_err(storage)?;
            let ids = statement.query_map(params![root.to_string()], |row| row.get::<_, String>(0)).map_err(storage)?.map(|value| value.map_err(storage).and_then(|value| parse_id(&value, "PlaylistId"))).collect();
            ids
        })
    }

    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        let name = name.to_owned();
        self.writer
            .run(move |connection| create_playlist(connection, id, root, &name))
    }

    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        let name = to_normalized_name.to_owned();
        self.writer.run(move |connection| {
            let key = playlist_name_key(&name);
            connection.execute("UPDATE playlists SET display_name = ?2, normalized_name_key = ?3, updated_at = ?4 WHERE uuid = ?1", params![id.to_string(), name, key, now_ms()]).map_err(map_constraint)?;
            Ok(())
        })
    }

    fn delete(&self, id: PlaylistId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT ps.playlist_uuid, ps.song_uuid, ps.position, s.availability FROM playlist_songs ps JOIN songs s ON s.uuid = ps.song_uuid WHERE ps.playlist_uuid = ?1 ORDER BY ps.position, ps.song_uuid").map_err(storage)?;
            let members = statement.query_map(params![id.to_string()], |row| {
                Ok(PlaylistMember::new(parse_id(&row.get::<_, String>(0)?, "PlaylistId").map_err(to_sql_error)?, parse_id(&row.get::<_, String>(1)?, "SongId").map_err(to_sql_error)?, row.get::<_, u64>(2)?, availability_from_db(&row.get::<_, String>(3)?).map_err(to_sql_error)?))
            }).map_err(storage)?.collect::<Result<Vec<_>, _>>().map_err(storage)?;
            Ok(members)
        })
    }

    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error> {
        self.writer
            .run(move |connection| add_member(connection, playlist, song, position))
    }

    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
                    params![playlist.to_string(), song.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }
}

impl OperationJournalRepository for SqliteDatabase {
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        let item = item.to_owned();
        self.with_reader(move |connection| operation_item(connection, operation, &item))
    }

    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.writer
            .run(move |connection| upsert_operation_item(connection, operation, item))
    }

    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT kind, state, song_uuid, target_relative_path, expected_hash, normalized_target_path FROM operation_items WHERE operation_uuid = ?1 ORDER BY item_key").map_err(storage)?;
            let items = statement.query_map(params![operation.to_string()], operation_item_from_row).map_err(storage)?.collect::<Result<Vec<_>, _>>().map_err(storage)?;
            Ok(items)
        })
    }
}

impl UnitOfWork for SqliteDatabase {
    fn with_tx<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut dyn TxAccess) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            let result = {
                let mut access = SqliteTx {
                    transaction: &transaction,
                };
                f(&mut access)
            }?;
            transaction.commit().map_err(storage)?;
            Ok(result)
        })
    }
}

struct SqliteTx<'connection> {
    transaction: &'connection Transaction<'connection>,
}

impl TxAccess for SqliteTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
        upsert_song(self.transaction, song)
    }
    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error> {
        set_song_availability(self.transaction, id, availability)
    }
    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error> {
        set_song_favorite(self.transaction, id, favorite)
    }
    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error> {
        increment_play_count(self.transaction, id)
    }
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error> {
        upsert_root(self.transaction, root)
    }
    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error> {
        create_playlist(self.transaction, id, root, name)
    }
    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error> {
        add_member(
            self.transaction,
            member.playlist(),
            member.song(),
            member.position(),
        )
    }
    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.transaction
            .execute(
                "DELETE FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
                params![playlist.to_string(), song.to_string()],
            )
            .map_err(storage)?;
        Ok(())
    }
    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error> {
        upsert_operation_item(self.transaction, operation, item)
    }
}
