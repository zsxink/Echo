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

use crate::application::ports::{CoverAssetRef, OperationItem, OperationResourceKind};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, RootAvailability, Song, SongAvailability,
};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use crate::domain::state::scan::ScanProgress;
use crate::error::Error;

use super::conversion::{availability_to_db, operation_state_to_db};
use super::support::{map_constraint, now_ms, parse_id, storage, to_sql_error};
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
    // The upsert carries *parsed metadata* + scan facts, never user state: a
    // scan (or any caller) may hold a stale snapshot, so `is_favorite`,
    // `play_count` and `availability` are intentionally absent from DO UPDATE.
    // Those columns can only change through their dedicated mutations, and a
    // concurrent scan writing back must never roll a favorite or play count
    // backward.
    connection.execute("INSERT INTO songs (uuid, library_root_uuid, relative_path, normalized_relative_path, title, artist, album, title_sort, artist_sort, album_sort, duration_ms, is_favorite, play_count, added_at, availability, revision, blake3_hash, file_size, file_mtime_ns, format, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?21) ON CONFLICT(uuid) DO UPDATE SET library_root_uuid = excluded.library_root_uuid, relative_path = excluded.relative_path, normalized_relative_path = excluded.normalized_relative_path, title = excluded.title, artist = excluded.artist, album = excluded.album, title_sort = excluded.title_sort, artist_sort = excluded.artist_sort, album_sort = excluded.album_sort, duration_ms = excluded.duration_ms, revision = excluded.revision, blake3_hash = excluded.blake3_hash, file_size = excluded.file_size, file_mtime_ns = excluded.file_mtime_ns, format = excluded.format, updated_at = excluded.updated_at", params![song.id().to_string(), song.root().to_string(), song.path().display(), song.path().identity_key(), title, artist, album, normalized_key(song.title().unwrap_or("")), normalized_key(song.artist().unwrap_or("")), normalized_key(song.album().unwrap_or("")), song.duration().map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)), i64::from(song.favorite()), i64::try_from(song.play_count().as_u64()).unwrap_or(i64::MAX), i64::try_from(song.added_at()).unwrap_or(i64::MAX), availability_to_db(song.availability()), i64::try_from(song.revision().as_u64()).unwrap_or(i64::MAX), song.blake3_hash(), song.file_size().map(|size| i64::try_from(size).unwrap_or(i64::MAX)), song.file_mtime_ns(), song.format().map(super::conversion::audio_format_to_db), now]).map_err(map_constraint)?;
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
    connection.query_row("SELECT i.kind, i.state, COALESCE(i.song_uuid, j.reserved_song_uuid), i.target_relative_path, i.expected_hash, i.normalized_target_path, i.source_locator, i.staging_relative_path FROM operation_items i JOIN operation_journal j ON j.operation_uuid = i.operation_uuid WHERE i.operation_uuid = ?1 AND i.item_key = ?2", params![operation.to_string(), item], super::conversion::operation_item_from_row).optional().map_err(storage)
}
/// Idempotently create the operation envelope (the journal's total-state row
/// every per-resource item attaches to; design §8). Repeats are no-ops so
/// recovery can re-run freely.
pub(crate) fn ensure_operation_journal(
    connection: &Connection,
    operation: OperationId,
    root: LibraryRootId,
    kind: &str,
    reserved_song: Option<SongId>,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO operation_journal (operation_uuid, library_root_uuid, kind, state, reserved_song_uuid, created_at, updated_at) VALUES (?1, ?2, ?3, 'planned', ?4, ?5, ?5) ON CONFLICT(operation_uuid) DO NOTHING",
            params![operation.to_string(), root.to_string(), kind, reserved_song.map(|id| id.to_string()), now_ms()],
        )
        .map_err(map_constraint)?;
    Ok(())
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
    // The item's `song_uuid` column carries a foreign key into `songs`. An
    // import's *reserved* SongId has no songs row until DatabaseCommitted, so
    // the reservation lives in the envelope's `reserved_song_uuid` (no FK)
    // and the item column is only populated once it can be satisfied; reads
    // COALESCE the two, so the port always reports the reserved identity.
    let song_uuid: Option<String> = match item.song {
        Some(id) => {
            let id = id.to_string();
            let stored: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM songs WHERE uuid = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            (stored > 0).then_some(id)
        }
        None => None,
    };
    connection.execute("INSERT INTO operation_items (operation_uuid, item_key, library_root_uuid, kind, state, song_uuid, source_locator, staging_relative_path, target_relative_path, normalized_target_path, expected_hash, claim_active) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1) ON CONFLICT(operation_uuid, item_key) DO UPDATE SET state = excluded.state, song_uuid = excluded.song_uuid, source_locator = excluded.source_locator, staging_relative_path = excluded.staging_relative_path, target_relative_path = excluded.target_relative_path, normalized_target_path = excluded.normalized_target_path, expected_hash = excluded.expected_hash", params![operation.to_string(), item.claim_key, root, if item.kind == OperationResourceKind::Audio { "audio" } else { "lyrics" }, operation_state_to_db(item.state), song_uuid, item.source, item.staging_path.map(|path| path.display().to_owned()), item.target_path.display(), item.target_path.identity_key(), item.expected_hash]).map_err(map_constraint)?;
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

/// Every journal item of `root` that has not reached a terminal state — the
/// recovery-input set (tasks 5.5 / 5.10). Terminal states (`Completed`,
/// `RolledBack`, `DatabaseFinalized`, `Restored`) are excluded; everything
/// else still has durable work to do (or a conflict to surface).
pub(crate) fn incomplete_operation_items(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<(OperationId, String, OperationItem)>, Error> {
    // The item columns (0–7) match `operation_item_from_row`; the operation
    // uuid and envelope kind are read off the trailing columns.
    let mut statement = connection
        .prepare(
            "SELECT i.kind, i.state, COALESCE(i.song_uuid, j.reserved_song_uuid), i.target_relative_path, i.expected_hash, i.normalized_target_path, i.source_locator, i.staging_relative_path, i.operation_uuid, j.kind FROM operation_items i JOIN operation_journal j ON j.operation_uuid = i.operation_uuid WHERE i.library_root_uuid = ?1 AND i.state NOT IN ('completed', 'rolled_back', 'database_finalized', 'restored') ORDER BY i.operation_uuid, i.item_key",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            let item = super::conversion::operation_item_from_row(row)?;
            let operation = row.get::<_, String>(8)?;
            let kind = row.get::<_, String>(9)?;
            Ok((operation, kind, item))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    rows.into_iter()
        .map(|(operation, kind, item)| {
            let operation = parse_id(&operation, "OperationId")?;
            Ok((operation, kind, item))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Phase 4: lyrics candidates, cover references, scan runs
// ---------------------------------------------------------------------------

/// All songs of one root (the scan identity snapshot). No LIMIT: scans need
/// the whole picture; per-row conversion keeps the memory shape domain-side.
pub(crate) fn all_songs_in_root(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<Song>, Error> {
    let mut statement = connection
        .prepare(&format!(
            "{} WHERE s.library_root_uuid = ?1 ORDER BY s.uuid",
            super::query::SONG_SELECT
        ))
        .map_err(storage)?;
    let songs = statement
        .query_map(params![root.to_string()], super::conversion::song_from_row)
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    Ok(songs)
}

/// Upsert one `(song, source)` lyrics candidate row, keyed by the candidate's
/// own source. Override rows are written only by override use cases — a scan
/// never constructs an override candidate.
pub(crate) fn set_lyrics_candidate(
    connection: &Connection,
    song: SongId,
    candidate: &LyricsCandidate,
) -> Result<(), Error> {
    connection.execute(
        "INSERT INTO song_lyrics (song_uuid, source, text_kind, raw_text, timed_lines_json, parse_error, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(song_uuid, source) DO UPDATE SET text_kind = excluded.text_kind, raw_text = excluded.raw_text, timed_lines_json = excluded.timed_lines_json, parse_error = excluded.parse_error, updated_at = excluded.updated_at",
        params![
            song.to_string(),
            lyrics_source_to_db(candidate.source()),
            super::conversion::text_kind_to_db(candidate),
            if candidate.raw_text().is_empty() { None } else { Some(candidate.raw_text()) },
            super::conversion::timed_lines_to_json(candidate),
            candidate.parse_error(),
            now_ms(),
        ],
    ).map_err(map_constraint)?;
    Ok(())
}

/// Remove one source's candidate row (stale embedded/sidecar text).
pub(crate) fn clear_lyrics_candidate(
    connection: &Connection,
    song: SongId,
    source: LyricsSource,
) -> Result<(), Error> {
    connection
        .execute(
            "DELETE FROM song_lyrics WHERE song_uuid = ?1 AND source = ?2",
            params![song.to_string(), lyrics_source_to_db(source)],
        )
        .map_err(storage)?;
    Ok(())
}

/// Every stored candidate of one song, all sources.
pub(crate) fn lyrics_candidates(
    connection: &Connection,
    song: SongId,
) -> Result<Vec<LyricsCandidate>, Error> {
    let mut statement = connection
        .prepare(
            "SELECT source, text_kind, raw_text, timed_lines_json, parse_error FROM song_lyrics WHERE song_uuid = ?1",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(params![song.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    rows.into_iter()
        .map(|(source, kind, raw, timed, parse_error)| {
            let source = super::conversion::lyrics_source_from_db(&source).map_err(to_sql_error)?;
            let lines = timed
                .as_deref()
                .map(super::conversion::timed_lines_from_json)
                .transpose()
                .map_err(to_sql_error)?
                .unwrap_or_default();
            // `plain` rows keep their readable text in raw_text; `empty` rows
            // are user clearings whose block-fallback semantics must survive
            // the round-trip.
            let plain_text = (kind == "plain").then(|| raw.clone().unwrap_or_default());
            let mut candidate = LyricsCandidate::with_raw_text(
                source,
                raw.unwrap_or_default(),
                lines,
                plain_text,
                parse_error,
            );
            if kind == "empty" {
                candidate.mark_empty_override();
            }
            Ok(candidate)
        })
        .collect::<rusqlite::Result<Vec<LyricsCandidate>>>()
        .map_err(storage)
}

const fn lyrics_source_to_db(source: LyricsSource) -> &'static str {
    match source {
        LyricsSource::Override => "override",
        LyricsSource::Embedded => "embedded",
        LyricsSource::Sidecar => "sidecar",
    }
}

/// Upsert the `cover_assets` row and point the song at it, in the caller's
/// transaction (task 4.6: cache key and database reference stay consistent).
pub(crate) fn attach_cover(
    connection: &Connection,
    song: SongId,
    cover: &CoverAssetRef,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO cover_assets (content_hash, mime_type, asset_key, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(content_hash) DO UPDATE SET mime_type = excluded.mime_type, asset_key = excluded.asset_key",
            params![cover.content_hash, cover.mime, cover.asset_key, now_ms()],
        )
        .map_err(map_constraint)?;
    connection
        .execute(
            "UPDATE songs SET cover_hash = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![song.to_string(), cover.content_hash, now_ms()],
        )
        .map_err(storage)?;
    Ok(())
}

/// The cover reference of one song, if any.
pub(crate) fn cover_of_song(
    connection: &Connection,
    song: SongId,
) -> Result<Option<CoverAssetRef>, Error> {
    connection
        .query_row(
            "SELECT ca.content_hash, ca.mime_type, ca.asset_key FROM songs s JOIN cover_assets ca ON ca.content_hash = s.cover_hash WHERE s.uuid = ?1",
            params![song.to_string()],
            |row| {
                Ok(CoverAssetRef {
                    content_hash: row.get(0)?,
                    mime: row.get(1)?,
                    asset_key: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(storage)
}

/// Every asset key referenced by any song of the root — the GC keep-set.
pub(crate) fn referenced_asset_keys(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<String>, Error> {
    let mut statement = connection
        .prepare(
            "SELECT ca.asset_key FROM songs s JOIN cover_assets ca ON ca.content_hash = s.cover_hash WHERE s.library_root_uuid = ?1",
        )
        .map_err(storage)?;
    let keys = statement
        .query_map(params![root.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    Ok(keys)
}

/// Open a `scan_runs` row for `(root, generation)`.
pub(crate) fn begin_scan_run(
    connection: &Connection,
    root: LibraryRootId,
    generation: u64,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO scan_runs (uuid, library_root_uuid, generation, state, started_at) VALUES (?1, ?2, ?3, 'queued', ?4)",
            params![
                scan_run_key(root, generation),
                root.to_string(),
                i64::try_from(generation).unwrap_or(i64::MAX),
                now_ms(),
            ],
        )
        .map_err(map_constraint)?;
    Ok(())
}

/// Persist a throttled progress snapshot.
pub(crate) fn update_scan_progress(
    connection: &Connection,
    root: LibraryRootId,
    generation: u64,
    progress: &ScanProgress,
) -> Result<(), Error> {
    let updated = connection
        .execute(
            "UPDATE scan_runs SET state = ?2, discovered_count = ?3, processed_count = ?4, created_count = ?5, updated_count = ?6, missing_count = ?7, skipped_count = ?8, failed_count = ?9 WHERE uuid = ?1",
            params![
                scan_run_key(root, generation),
                scan_state_to_db(progress.state),
                i64::try_from(progress.discovered).unwrap_or(i64::MAX),
                i64::try_from(progress.processed).unwrap_or(i64::MAX),
                i64::try_from(progress.created).unwrap_or(i64::MAX),
                i64::try_from(progress.updated).unwrap_or(i64::MAX),
                i64::try_from(progress.missing).unwrap_or(i64::MAX),
                i64::try_from(progress.skipped).unwrap_or(i64::MAX),
                i64::try_from(progress.failed).unwrap_or(i64::MAX),
            ],
        )
        .map_err(storage)?;
    if updated == 0 {
        return Err(Error::InvariantViolation {
            why: "progress update for an unknown scan run".to_owned(),
        });
    }
    Ok(())
}

/// Record one per-file issue.
pub(crate) fn record_scan_issue(
    connection: &Connection,
    root: LibraryRootId,
    generation: u64,
    issue: &crate::domain::entities::MediaDiagnostic,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO scan_issues (scan_run_uuid, relative_path, code, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                scan_run_key(root, generation),
                issue.path().display(),
                issue.code(),
                issue.reason(),
                now_ms(),
            ],
        )
        .map_err(storage)?;
    Ok(())
}

/// Persist the terminal state and final summary snapshot.
pub(crate) fn finish_scan_run(
    connection: &Connection,
    root: LibraryRootId,
    generation: u64,
    state: crate::domain::state::scan::ScanState,
    progress: &ScanProgress,
) -> Result<(), Error> {
    let updated = connection
        .execute(
            "UPDATE scan_runs SET state = ?2, discovered_count = ?3, processed_count = ?4, created_count = ?5, updated_count = ?6, missing_count = ?7, skipped_count = ?8, failed_count = ?9, finished_at = ?10 WHERE uuid = ?1",
            params![
                scan_run_key(root, generation),
                scan_state_to_db(state),
                i64::try_from(progress.discovered).unwrap_or(i64::MAX),
                i64::try_from(progress.processed).unwrap_or(i64::MAX),
                i64::try_from(progress.created).unwrap_or(i64::MAX),
                i64::try_from(progress.updated).unwrap_or(i64::MAX),
                i64::try_from(progress.missing).unwrap_or(i64::MAX),
                i64::try_from(progress.skipped).unwrap_or(i64::MAX),
                i64::try_from(progress.failed).unwrap_or(i64::MAX),
                now_ms(),
            ],
        )
        .map_err(storage)?;
    if updated == 0 {
        return Err(Error::InvariantViolation {
            why: "finish for an unknown scan run".to_owned(),
        });
    }
    Ok(())
}

/// The newest generation recorded for the root.
pub(crate) fn latest_scan_generation(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Option<u64>, Error> {
    let value: Option<i64> = connection
        .query_row(
            "SELECT MAX(generation) FROM scan_runs WHERE library_root_uuid = ?1",
            params![root.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .flatten();
    Ok(value.and_then(|value| u64::try_from(value).ok()))
}

/// Runtime key/value read (`runtime_state` table; `root_epoch` today).
pub(crate) fn load_runtime_state(
    connection: &Connection,
    key: &str,
) -> Result<Option<String>, Error> {
    connection
        .query_row(
            "SELECT value FROM runtime_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)
}

/// Runtime key/value write.
pub(crate) fn store_runtime_state(
    connection: &Connection,
    key: &str,
    value: &str,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO runtime_state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(storage)?;
    Ok(())
}

/// The durable row key of a `(root, generation)` run. Runs are rooted at the
/// unique `(library_root_uuid, generation)` index; the primary key is a
/// derived, stable text key so no extra identity type is needed.
fn scan_run_key(root: LibraryRootId, generation: u64) -> String {
    format!("{root}:{generation}")
}

const fn scan_state_to_db(state: crate::domain::state::scan::ScanState) -> &'static str {
    match state {
        crate::domain::state::scan::ScanState::Queued => "queued",
        crate::domain::state::scan::ScanState::Enumerating => "enumerating",
        crate::domain::state::scan::ScanState::Parsing => "parsing",
        crate::domain::state::scan::ScanState::Reconciling => "reconciling",
        crate::domain::state::scan::ScanState::Completed => "completed",
        crate::domain::state::scan::ScanState::Cancelled => "cancelled",
        crate::domain::state::scan::ScanState::Failed => "failed",
    }
}
