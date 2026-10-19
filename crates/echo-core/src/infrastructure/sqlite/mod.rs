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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Condvar, Mutex};

use rusqlite::params;
use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::application::ports::{
    CatalogQueryRepository, CoverAssetRef, CoverRepository, LibraryRepository, LyricsRepository,
    OperationItem, OperationJournalRepository, PlaylistRepository, RootWriteRepository,
    RuntimeStateStore, ScanRunRepository, SongRepository, TxLyricsWriter, TxOperationWriter,
    TxPlaylistWriter, TxRootWriter, TxSongWriter, TxStateWriter, TxWork, UnitOfWork,
};
use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort, RECENT_VIEW_LIMIT};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::{
    LibraryRootId, OperationId, PlaybackSessionId, PlaylistId, RelativeMediaPath, SongId,
};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::domain::text::{normalized_key, playlist_name_key};
use crate::error::{Error, Subject};

pub(crate) mod actor;
pub(crate) mod connection;
pub(crate) mod conversion;
pub(crate) mod device_state;
pub(crate) mod query;
mod remote_resources;
mod resource_retention;
pub(crate) mod statements;
pub(crate) mod support;
pub(crate) mod sync;
pub(crate) mod sync_apply;
mod sync_baseline;
pub(crate) mod sync_configuration;
mod sync_conflicts;
mod sync_context;
pub mod sync_identity_repair;
pub(crate) mod sync_ledger;
pub(crate) mod sync_manifests;
pub(crate) mod sync_projection;
pub(crate) mod sync_queue;
mod sync_recovery;
mod sync_snapshot;
mod sync_song_aliases;
#[cfg(test)]
mod tests;

use actor::SqliteWriteActor;
#[cfg(test)]
use connection::apply_migration_set;
use connection::{
    apply_migrations, backup_connection, backup_path, file_is_non_empty, open_reader, open_writer,
    quick_check_connection, reader_count,
};
use conversion::{
    availability_from_db, operation_item_from_row, root_from_row, scan_state_from_db, song_from_row,
};
use query::{active_root_id, catalog_counts, playlist_songs_query, query_active, SONG_SELECT};
use statements::{
    add_member, all_referenced_asset_keys, all_songs_in_root, attach_cover, begin_scan_run,
    clear_lyrics_candidate, cover_of_song, create_playlist, delete_song, ensure_operation_journal,
    finish_scan_run, incomplete_operation_items, increment_play_count, latest_scan_generation,
    load_runtime_state, lyrics_candidates, operation_item, record_scan_issue,
    referenced_asset_keys, release_operation_claims, set_lyrics_candidate, set_play_count,
    set_song_availability, set_song_favorite, store_runtime_state, update_scan_progress,
    upsert_member, upsert_operation_item, upsert_root, upsert_song,
};
use support::{map_constraint, now_ms, parse_id, storage, to_sql_error};

/// Bounded pool of read-only WAL connections. `acquire` blocks while every
/// pooled connection is checked out, so concurrent readers can never grow the
/// pool beyond [`reader_count()`] (2–4 connections per the design); opening a
/// fresh connection on demand would defeat the bound.
struct ReaderPool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

impl ReaderPool {
    const fn new(connections: Vec<Connection>) -> Self {
        Self {
            idle: Mutex::new(connections),
            available: Condvar::new(),
        }
    }

    fn acquire(&self) -> Result<Connection, Error> {
        let mut idle = self.idle.lock().map_err(|_| Error::InvariantViolation {
            why: "SQLite reader pool poisoned".to_owned(),
        })?;
        loop {
            if let Some(connection) = idle.pop() {
                return Ok(connection);
            }
            idle = self
                .available
                .wait(idle)
                .map_err(|_| Error::InvariantViolation {
                    why: "SQLite reader pool poisoned".to_owned(),
                })?;
        }
    }

    fn release(&self, connection: Connection) -> Result<(), Error> {
        self.idle
            .lock()
            .map_err(|_| Error::InvariantViolation {
                why: "SQLite reader pool poisoned".to_owned(),
            })?
            .push(connection);
        self.available.notify_one();
        Ok(())
    }
}

/// Real SQLite implementation of the library repositories and unit of work.
///
/// `open` applies ordered migrations before the actor starts, then owns one
/// write connection plus three read-only WAL connections. The type exposes no
/// `rusqlite` connection to application callers.
pub struct SqliteDatabase {
    path: PathBuf,
    writer: SqliteWriteActor,
    readers: ReaderPool,
    /// The single stable identity of this device (migration 0006), loaded once
    /// at open. Portable records are stamped with it.
    device_id: crate::domain::library::DeviceId,
}

fn read_device_play_counts(
    connection: &Connection,
    song: SongId,
) -> Result<BTreeMap<uuid::Uuid, u64>, Error> {
    let mut statement = connection
        .prepare("SELECT device_id, play_count FROM sync_device_play_counts WHERE song_uuid = ?1 ORDER BY device_id")
        .map_err(storage)?;
    let rows = statement
        .query_map(params![song.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    let mut counts = BTreeMap::new();
    for (device, count) in rows {
        let id = uuid::Uuid::parse_str(&device).map_err(|_| Error::InvariantViolation {
            why: "invalid device UUID in play-count table".to_owned(),
        })?;
        counts.insert(id, u64::try_from(count).unwrap_or(0));
    }
    Ok(counts)
}

mod db_inherent;
mod repos_catalog;
mod repos_misc;
mod tx_impl;

#[cfg(test)]
mod write_wakeup_tests;
