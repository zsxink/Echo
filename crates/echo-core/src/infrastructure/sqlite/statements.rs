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
    LibraryRoot, LyricsCandidate, LyricsSource, PlaylistMember, RootAvailability, Song,
    SongAvailability,
};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use crate::domain::library::HybridLogicalClock;
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
    connection.execute("INSERT INTO library_roots (uuid, absolute_path, normalized_path_key, is_active, write_capable, write_safety_locked, availability, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) ON CONFLICT(uuid) DO UPDATE SET absolute_path = excluded.absolute_path, normalized_path_key = excluded.normalized_path_key, is_active = excluded.is_active, write_capable = excluded.write_capable, write_safety_locked = library_roots.write_safety_locked OR excluded.write_safety_locked, availability = excluded.availability, updated_at = excluded.updated_at", params![root.id().to_string(), path, key, i64::from(root.is_active()), i64::from(root.observed_write_capable()), i64::from(root.write_safety_locked()), if root.availability() == RootAvailability::Available { "available" } else { "unavailable" }, now]).map_err(map_constraint)?;
    if root.is_active() {
        super::sync_context::select(connection, &root.id().to_string())?;
    }
    Ok(())
}

/// Read shared song facts from the post-mutation state in the caller's
/// transaction. Favorite and play counts have independent object identities;
/// availability, mtime and directory mapping belong to this device only.
/// Resource manifests and bytes are captured by the resource pipeline.
pub(crate) fn song_full_snapshot_payload(
    connection: &Connection,
    id: SongId,
) -> Result<serde_json::Value, Error> {
    connection
        .query_row(
            "SELECT COALESCE((SELECT l.library_relative_path FROM sync_remote_resource_locations l
                JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
                WHERE l.local_root_uuid=songs.library_root_uuid AND l.object_type='song'
                  AND l.object_uuid=songs.uuid AND l.local_relative_path=songs.relative_path
                  AND p.completed=1 LIMIT 1),relative_path),
                title, artist, album, blake3_hash, file_size, duration_ms, format, bitrate_bps,
                sample_rate_hz, channels, bits_per_sample, added_at FROM songs WHERE uuid = ?1",
            params![id.to_string()],
            |row| {
                let relative_path: String = row.get(0)?;
                let title: Option<String> = row.get(1)?;
                let artist: Option<String> = row.get(2)?;
                let album: Option<String> = row.get(3)?;
                let blake3_hash: Option<String> = row.get(4)?;
                let file_size: Option<i64> = row.get(5)?;
                let duration_ms: Option<i64> = row.get(6)?;
                let format: Option<String> = row.get(7)?;
                let bitrate_bps: Option<i64> = row.get(8)?;
                let sample_rate_hz: Option<i64> = row.get(9)?;
                let channels: Option<i64> = row.get(10)?;
                let bits_per_sample: Option<i64> = row.get(11)?;
                let added_at: i64 = row.get(12)?;
                Ok(serde_json::json!({
                    "song_uuid": id.to_string(),
                    "relative_path": relative_path,
                    "title": title,
                    "artist": artist,
                    "album": album,
                    "blake3_hash": blake3_hash,
                    "file_size": file_size,
                    "duration_ms": duration_ms,
                    "format": format,
                    "bitrate_bps": bitrate_bps,
                    "sample_rate_hz": sample_rate_hz,
                    "channels": channels,
                    "bits_per_sample": bits_per_sample,
                    "added_at": added_at,
                }))
            },
        )
        .map_err(storage)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn upsert_song(connection: &Connection, song: &Song) -> Result<(), Error> {
    super::sync_projection::ensure_song_live(connection, &song.id().to_string())?;
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
    connection.execute("INSERT INTO songs (uuid, library_root_uuid, relative_path, normalized_relative_path, title, artist, album, title_sort, artist_sort, album_sort, duration_ms, is_favorite, play_count, added_at, availability, revision, blake3_hash, file_size, file_mtime_ns, format, bitrate_bps, sample_rate_hz, channels, bits_per_sample, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?25) ON CONFLICT(uuid) DO UPDATE SET library_root_uuid = excluded.library_root_uuid, relative_path = excluded.relative_path, normalized_relative_path = excluded.normalized_relative_path, title = excluded.title, artist = excluded.artist, album = excluded.album, title_sort = excluded.title_sort, artist_sort = excluded.artist_sort, album_sort = excluded.album_sort, duration_ms = excluded.duration_ms, revision = excluded.revision, blake3_hash = excluded.blake3_hash, file_size = excluded.file_size, file_mtime_ns = excluded.file_mtime_ns, format = excluded.format, bitrate_bps = excluded.bitrate_bps, sample_rate_hz = excluded.sample_rate_hz, channels = excluded.channels, bits_per_sample = excluded.bits_per_sample, updated_at = excluded.updated_at", params![song.id().to_string(), song.root().to_string(), song.path().display(), song.path().identity_key(), title, artist, album, normalized_key(song.title().unwrap_or("")), normalized_key(song.artist().unwrap_or("")), normalized_key(song.album().unwrap_or("")), song.duration().map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)), i64::from(song.favorite()), i64::try_from(song.play_count().as_u64()).unwrap_or(i64::MAX), i64::try_from(song.added_at()).unwrap_or(i64::MAX), availability_to_db(song.availability()), i64::try_from(song.revision().as_u64()).unwrap_or(i64::MAX), song.blake3_hash(), song.file_size().map(|size| i64::try_from(size).unwrap_or(i64::MAX)), song.file_mtime_ns(), song.format().map(super::conversion::audio_format_to_db), song.audio_parameters().bitrate_bps.and_then(|v| i64::try_from(v).ok()), song.audio_parameters().sample_rate_hz.map(i64::from), song.audio_parameters().channels.map(i64::from), song.audio_parameters().bits_per_sample.map(i64::from), now]).map_err(map_constraint)?;
    maintain_search(connection, song)?;
    touch_root(connection, song.root())?;
    // Sync foundation (task 3.11): a song write is a syncable fact — append its
    // full snapshot to outbox in this same transaction. The snapshot is read
    // back from the post-write state so it is always authoritative; payload is
    // relative path + fields only, never an absolute path.
    let id = song.id().to_string();
    let payload = song_full_snapshot_payload(connection, song.id())?;
    let _revision = enqueue_object_snapshot(
        connection,
        super::sync::KIND_SONG,
        &id,
        &payload,
        super::sync::OP_UPSERT,
    )?;
    // Stamp the object's HLC (migration 0006) so the portable record built
    // after the commit carries the same clock the DB row holds.
    let now_secs = u64::try_from(now_ms().max(0)).unwrap_or(0) / 1_000;
    let _ = advance_object_hlc(connection, super::sync::KIND_SONG, &id, now_secs)?;
    Ok(())
}

/// Only local file facts change after parsing an already-authorized sync file.
/// Guard the association so a concurrent delete/replacement cannot receive
/// artwork or lyrics parsed from its previous bytes.
pub(crate) fn refresh_song_file_index(connection: &Connection, song: &Song) -> Result<(), Error> {
    let parameters = song.audio_parameters();
    let changed = connection
        .execute(
            "UPDATE songs SET file_mtime_ns=?1, format=?2, bitrate_bps=?3, availability='available',
           sample_rate_hz=?4, channels=?5, bits_per_sample=?6
         WHERE uuid=?7 AND library_root_uuid=?8 AND normalized_relative_path=?9
           AND blake3_hash=?10 AND file_size=?11
           AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=songs.uuid)
           AND NOT EXISTS(SELECT 1 FROM sync_object_versions v
             WHERE v.object_type='song' AND v.object_uuid=songs.uuid AND v.is_tombstone=1)
           AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents d
             WHERE d.object_type='song' AND d.object_uuid=songs.uuid AND d.state='open')
           AND (availability='available' OR (availability='missing'
             AND EXISTS(SELECT 1 FROM sync_binding b WHERE b.singleton=1
               AND b.local_root_uuid=songs.library_root_uuid)
             AND NOT EXISTS(SELECT 1 FROM sync_mutations m
               WHERE m.object_type='song' AND m.object_uuid=songs.uuid AND m.status='pending')
             AND EXISTS(SELECT 1 FROM sync_remote_resource_locations l
               JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
               JOIN sync_resource_manifests m ON m.object_type=l.object_type
                 AND m.object_uuid=l.object_uuid AND m.library_relative_path=l.library_relative_path
                 AND m.content_hash=l.content_hash AND m.ref_kind='audio'
               WHERE l.local_root_uuid=songs.library_root_uuid AND l.object_type='song'
                 AND l.object_uuid=songs.uuid AND lower(hex(l.content_hash))=songs.blake3_hash
                 AND l.local_relative_path=songs.relative_path AND p.completed=1)))",
            params![
                song.file_mtime_ns(),
                song.format().map(super::conversion::audio_format_to_db),
                parameters
                    .bitrate_bps
                    .and_then(|value| i64::try_from(value).ok()),
                parameters.sample_rate_hz.map(i64::from),
                parameters.channels.map(i64::from),
                parameters.bits_per_sample.map(i64::from),
                song.id().to_string(),
                song.root().to_string(),
                song.path().identity_key(),
                song.blake3_hash(),
                song.file_size().and_then(|value| i64::try_from(value).ok()),
            ],
        )
        .map_err(storage)?;
    if changed != 1 {
        return Err(Error::conflict(
            "published song association changed during metadata indexing",
        ));
    }
    if song.availability() == SongAvailability::Missing {
        // Recovery changes only this device's read model. Keep the confirmed
        // shared tags in the same transaction as availability and parsed assets.
        if let Some(payload) = connection.query_row(
            "SELECT payload_json FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1 AND is_tombstone=0",
            params![song.id().to_string()], |row| row.get::<_, String>(0),
        ).optional().map_err(storage)? {
            super::sync_projection::repair_confirmed_song_tags(
                connection, &song.id().to_string(), &serde_json::from_str(&payload).map_err(storage)?,
                &song.root().to_string(),
            )?;
        }
        touch_root(connection, song.root())?;
    }
    Ok(())
}

/// Append an outbox row from a full JSON snapshot of `kind`/uuid, deriving the
/// object's monotone revision from its outbox history and mirroring the ordinal
/// onto the canonical column (`playlists`/`library_roots`/`song_overrides`).
/// `songs.revision` is deliberately NOT touched (it is the persistence layer's
/// optimistic tag — see sync.rs module doc). The parses of `uuid` are not
/// needed: outbox stores opaque object UUIDs.
///
/// Returns the enqueued object revision so callers can stamp a portable record
/// (design §2: record and outbox share the same monotone revision).
pub(crate) fn enqueue_object_snapshot(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    payload: &serde_json::Value,
    operation: &str,
) -> Result<i64, Error> {
    // Every playlist write must capture the same complete post-write view.
    // Rename/member callers historically supplied different partial payloads,
    // which could erase membership or the name when replayed as an object.
    let playlist_payload;
    let payload = if kind == super::sync::KIND_PLAYLIST && operation == super::sync::OP_UPSERT {
        playlist_payload = playlist_full_snapshot_payload(
            connection,
            parse_id::<PlaylistId>(object_uuid, "PlaylistId")?,
        )?;
        &playlist_payload
    } else {
        payload
    };
    let json = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned());
    let revision = super::sync::enqueue_sync(connection, kind, object_uuid, &json, operation)?;
    // Mirror onto canonical columns (idempotent; only present after 0005).
    match kind {
        super::sync::KIND_PLAYLIST => {
            let id = parse_id::<PlaylistId>(object_uuid, "PlaylistId")?;
            super::sync::mirror_playlist_revision(connection, id, revision)?;
        }
        super::sync::KIND_OVERRIDE => {
            let song = parse_id::<SongId>(object_uuid, "SongId")?;
            super::sync::mirror_override_revision(connection, song, revision)?;
        }
        _ => {}
    }
    Ok(revision)
}

/// Advance the HLC column of one object's canonical row and return the new
/// clock. The 0006 columns (`songs`/`song_overrides`/`playlists`.`hlc_*`) are
/// the durable, per-object HLC; the portable record stamped after the commit
/// must share the value this returns (single source of truth).
pub(crate) fn advance_object_hlc(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    now_wall_secs: u64,
) -> Result<HybridLogicalClock, Error> {
    let (table, column) = match kind {
        super::sync::KIND_SONG => ("songs", "uuid"),
        super::sync::KIND_PLAYLIST => ("playlists", "uuid"),
        super::sync::KIND_OVERRIDE => ("song_overrides", "song_uuid"),
        _ => {
            // Unknown kinds carry no HLC column; default the new clock.
            return Ok(HybridLogicalClock::new(now_wall_secs, 0));
        }
    };
    let (prev_wall, prev_counter): (i64, i64) = connection
        .query_row(
            &format!("SELECT hlc_wall_secs, hlc_counter FROM {table} WHERE {column} = ?1"),
            [object_uuid.to_owned()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?
        .unwrap_or((0, 0));
    let prev = HybridLogicalClock::new(
        u64::try_from(prev_wall).unwrap_or(0),
        u32::try_from(prev_counter).unwrap_or(0),
    );
    let next = crate::domain::library::next_hlc(prev, now_wall_secs);
    connection
        .execute(
            &format!(
                "UPDATE {table} SET hlc_wall_secs = ?1, hlc_counter = ?2, updated_at = ?3 WHERE {column} = ?4"
            ),
            rusqlite::params![
                i64::try_from(next.wall_secs).unwrap_or(i64::MAX),
                i64::from(next.counter),
                now_ms(),
                object_uuid
            ],
        )
        .map_err(storage)?;
    Ok(next)
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
    let canonical = super::sync_song_aliases::resolve(connection, &id.to_string())?;
    let is_alias = canonical != id.to_string();
    if availability != SongAvailability::PendingDelete {
        super::sync_projection::ensure_song_live(connection, &id.to_string())?;
        super::sync_projection::ensure_song_live(connection, &canonical)?;
    }
    connection
        .execute(
            "UPDATE songs SET availability = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![
                id.to_string(),
                availability_to_db(if is_alias {
                    SongAvailability::PendingDelete
                } else {
                    availability
                }),
                now_ms()
            ],
        )
        .map_err(storage)?;
    if availability == SongAvailability::Available {
        connection
            .execute(
                "UPDATE sync_local_delete_intents SET state = 'cancelled' WHERE object_type = 'song' AND object_uuid = ?1 AND state = 'open'",
                params![id.to_string()],
            )
            .map_err(storage)?;
        // Undo restores this operation's original physical files and cancels
        // its local intent. An alias remains hidden: its placeholder path is
        // retained only for immutable queued requests, never a catalog row.
        if is_alias {
            return touch_root_for_song(connection, id);
        }
        let confirmed: Option<(Option<String>, bool)> = connection
            .query_row(
                "SELECT v.payload_json, v.is_tombstone FROM sync_object_versions v WHERE v.object_type='song' AND v.object_uuid=?1",
                params![id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        let local_root: Option<String> = connection
            .query_row(
                "SELECT local_root_uuid FROM sync_binding WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .flatten();
        if let (Some((Some(payload), false)), Some(local_root)) = (confirmed, local_root) {
            let payload: serde_json::Value = serde_json::from_str(&payload).map_err(storage)?;
            super::sync_projection::project_confirmed_song_state(
                connection,
                super::sync::KIND_SONG,
                &id.to_string(),
                &payload,
                Some(&local_root),
            )?;
        }
    }
    touch_root_for_song(connection, id)?;
    // Availability describes this device's media mount and undo workflow. It
    // is intentionally local-only; only irreversible delete finalization
    // writes a shared typed tombstone.
    Ok(())
}

pub(crate) fn delete_song(connection: &Connection, id: SongId) -> Result<(), Error> {
    super::sync_projection::ensure_song_live(connection, &id.to_string())?;
    // `delete_song` is reached only through an Echo-initiated delete finalization
    // (never an external missing — that keeps `availability = missing` and its
    // associations). So a delete here IS a syncable tombstone + delete-outbox
    // row; together they let the deletion propagate cross-device (task 3.12).
    let id_str = id.to_string();
    let stable_mutation_id: Option<String> = connection
        .query_row(
            "SELECT final_mutation_id FROM sync_local_delete_intents WHERE object_type = 'song' AND object_uuid = ?1 AND state = 'open' ORDER BY created_at DESC LIMIT 1",
            params![id_str],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    let payload = serde_json::json!({ "tombstone": true });
    let revision = super::sync::enqueue_sync_with_id(
        connection,
        super::sync::KIND_SONG,
        &id_str,
        &serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned()),
        super::sync::OP_DELETE,
        stable_mutation_id.as_deref(),
    )?;
    super::sync::write_tombstone(connection, super::sync::KIND_SONG, &id_str, revision)?;
    connection
        .execute(
            "UPDATE sync_local_delete_intents SET state = 'finalized', finalized_at = ?2 WHERE object_type = 'song' AND object_uuid = ?1 AND state = 'open'",
            params![id_str, now_ms()],
        )
        .map_err(storage)?;
    // FTS external-content tables are not necessarily cleaned by a foreign
    // key cascade. Remove the searchable row in the same finalization
    // transaction before deleting the canonical song record.
    connection
        .execute(
            "DELETE FROM song_search WHERE song_uuid = ?1",
            params![id.to_string()],
        )
        .map_err(storage)?;
    connection
        .execute("DELETE FROM songs WHERE uuid = ?1", params![id.to_string()])
        .map_err(storage)?;
    Ok(())
}
pub(crate) fn set_song_favorite(
    connection: &Connection,
    id: SongId,
    favorite: bool,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE songs
             SET is_favorite = ?2,
                 favorited_at = CASE
                     WHEN ?2 = 0 THEN NULL
                     WHEN is_favorite = 0 THEN ?3
                     ELSE favorited_at
                 END,
                 updated_at = ?3
             WHERE uuid = ?1",
            params![id.to_string(), i64::from(favorite), now_ms()],
        )
        .map_err(storage)?;
    touch_root_for_song(connection, id)?;
    // A favorite is its own portable object keyed by the song UUID. Keep it
    // separate from the song snapshot so a favorite-only mutation cannot
    // overwrite media metadata on another device. The canonical row and this
    // complete favorite payload still commit in the same transaction.
    let favorited_at: Option<i64> = connection
        .query_row(
            "SELECT favorited_at FROM songs WHERE uuid = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let payload = serde_json::json!({
        "song_uuid": id.to_string(),
        "is_favorite": favorite,
        "favorited_at": favorited_at,
    });
    enqueue_object_snapshot(
        connection,
        super::sync::KIND_FAVORITE,
        &id.to_string(),
        &payload,
        super::sync::OP_UPSERT,
    )?;
    // Stamp the song HLC so the favorite portable record shares the clock.
    let now_secs = u64::try_from(now_ms().max(0)).unwrap_or(0) / 1_000;
    let _ = advance_object_hlc(
        connection,
        super::sync::KIND_SONG,
        &id.to_string(),
        now_secs,
    )?;
    Ok(())
}
/// Restore a play count from a portable `play-stats` record. `MAX()` keeps the
/// counter monotone: a continuation can raise the count but never erase plays
/// this device already recorded, which also makes re-projection idempotent.
pub(crate) fn set_play_count(connection: &Connection, id: SongId, count: u64) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE songs SET play_count = MAX(play_count, ?2), updated_at = ?3 WHERE uuid = ?1",
            params![
                id.to_string(),
                i64::try_from(count).unwrap_or(i64::MAX),
                now_ms()
            ],
        )
        .map_err(storage)?;
    touch_root_for_song(connection, id)
}

/// Insert or refresh a playlist membership **by member UUID**. The
/// continuation path reuses the identity carried by the portable
/// `playlist-items` record; a fresh UUID per open would drift from the very
/// records being replayed (and from their tombstones).
mod operations;
pub(crate) use operations::*;

#[cfg(test)]
#[path = "tests/sync_payload_completeness.rs"]
mod sync_payload_completeness;
