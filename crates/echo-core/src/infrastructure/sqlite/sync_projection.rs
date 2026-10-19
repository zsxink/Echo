//! Materialize confirmed independent song state without creating local outbox
//! events. Pending local mutations remain the effective view until resolved.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

use crate::domain::ids::Revision;
use crate::domain::library::{DeviceId as LibraryDeviceId, HybridLogicalClock, PlayStatsRecord};
use crate::domain::text::{normalized_key, playlist_name_key};
use crate::error::Error;

use super::support::{now_ms, storage};

/// A confirmed deletion ends this identity even if a stale scanner or undo
/// operation still holds a copy of the old row.
pub(super) fn ensure_song_live(tx: &Connection, object_uuid: &str) -> Result<(), Error> {
    let archived: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM archived_sync_object_versions WHERE object_type='song' AND object_uuid=?1 AND is_tombstone=1)",
        params![object_uuid], |row| row.get(0),
    ).map_err(storage)?;
    if archived || is_tombstoned(tx, "song", object_uuid)? {
        return Err(Error::conflict(
            "song was finally deleted; recovery requires a new UUID",
        ));
    }
    Ok(())
}

pub(super) fn is_tombstoned(
    tx: &Connection,
    object_type: &str,
    object_uuid: &str,
) -> Result<bool, Error> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2 AND is_tombstone=1)",
        params![object_type, object_uuid],
        |row| row.get(0),
    )
    .map_err(storage)
}

pub(super) fn project_confirmed_tombstone(
    tx: &Connection,
    object_type: &str,
    object_uuid: &str,
) -> Result<(), Error> {
    match object_type {
        "song" => {
            super::resource_retention::plan_remote_delete(tx, object_uuid)?;
            // Retain the local row and journal evidence for explicit recovery.
            // PendingDelete already hides it from every catalog/queue view.
            tx.execute(
                "UPDATE songs SET availability='pending_delete', is_favorite=0, favorited_at=NULL, updated_at=?2 WHERE uuid=?1",
                params![object_uuid, now_ms()],
            ).map_err(storage)?;
            tx.execute(
                "DELETE FROM song_search WHERE song_uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM playlist_songs WHERE song_uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM song_overrides WHERE song_uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE sync_local_delete_intents SET state='remote_deleted', finalized_at=?2 WHERE object_type='song' AND (object_uuid=?1 OR object_uuid IN (SELECT alias_song_uuid FROM sync_song_aliases WHERE canonical_song_uuid=?1)) AND state='open'",
                params![object_uuid, now_ms()],
            ).map_err(storage)?;
            // Holding the journal prevents restart from forwarding these staged
            // resources to system trash after the former undo deadline. Aliases
            // retain their original local journals, so the canonical deletion
            // must hold those independent physical copies as well.
            tx.execute(
                "UPDATE operation_items SET state='failed_recoverable', error_code='sync_remote_deleted' WHERE operation_uuid IN (SELECT operation_uuid FROM operation_journal WHERE kind='delete' AND (reserved_song_uuid=?1 OR reserved_song_uuid IN (SELECT alias_song_uuid FROM sync_song_aliases WHERE canonical_song_uuid=?1))) AND state NOT IN ('database_finalized','rolled_back','completed')",
                params![object_uuid],
            ).map_err(storage)?;
        }
        "playlist" => {
            // Prior snapshots and resource references remain in the immutable
            // commit log; deleting only the read model removes all UI entry points.
            tx.execute("DELETE FROM playlists WHERE uuid=?1", params![object_uuid])
                .map_err(storage)?;
        }
        "playlist-item" => {
            tx.execute(
                "DELETE FROM playlist_songs WHERE member_uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
        }
        "favorite" => {
            tx.execute(
                "UPDATE songs SET is_favorite=0, favorited_at=NULL WHERE uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
        }
        "override" => {
            tx.execute(
                "DELETE FROM song_overrides WHERE song_uuid=?1",
                params![object_uuid],
            )
            .map_err(storage)?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn project_confirmed_song_state(
    tx: &Connection,
    object_type: &str,
    object_uuid: &str,
    payload: &Value,
    local_root_uuid: Option<&str>,
) -> Result<(), Error> {
    project_confirmed_state(
        tx,
        object_type,
        object_uuid,
        payload,
        local_root_uuid,
        false,
    )
}

pub(super) fn repair_confirmed_song_tags(
    tx: &Connection,
    song_uuid: &str,
    payload: &Value,
    root_uuid: &str,
) -> Result<(), Error> {
    project_confirmed_state(tx, "song", song_uuid, payload, Some(root_uuid), true)
}

fn project_confirmed_state(
    tx: &Connection,
    object_type: &str,
    object_uuid: &str,
    payload: &Value,
    local_root_uuid: Option<&str>,
    preserve_file_facts: bool,
) -> Result<(), Error> {
    if !matches!(
        object_type,
        "song" | "favorite" | "override" | "play-stats" | "playlist"
    ) {
        return Ok(());
    }
    if is_tombstoned(tx, object_type, object_uuid)?
        || (matches!(object_type, "favorite" | "override" | "play-stats")
            && is_tombstoned(tx, "song", object_uuid)?)
    {
        return Ok(());
    }
    let pending = if object_type == "play-stats" {
        false
    } else {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE object_type=?1 AND (object_uuid=?2 OR object_uuid IN (SELECT alias_song_uuid FROM sync_song_aliases WHERE canonical_song_uuid=?2)) AND status='pending')",
            params![object_type, object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?
    };
    if pending {
        return Ok(());
    }
    if object_type == "song" {
        return project_song(
            tx,
            object_uuid,
            payload,
            local_root_uuid,
            preserve_file_facts,
        );
    }
    if object_type == "playlist" {
        return project_playlist(tx, object_uuid, payload, local_root_uuid);
    }
    let song_uuid = payload
        .get("song_uuid")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict("confirmed song state is missing song_uuid"))?;
    if song_uuid != object_uuid {
        return Err(Error::conflict(
            "confirmed song state UUID does not match its object key",
        ));
    }
    let song_exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1)",
            params![song_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    // Object commits can precede the corresponding song/resource. The ledger
    // remains authoritative and enrollment/materialization can replay it once
    // the song row exists.
    if !song_exists && object_type != "play-stats" {
        return Ok(());
    }
    match object_type {
        "favorite" => {
            let favorite = payload
                .get("is_favorite")
                .and_then(Value::as_bool)
                .ok_or_else(|| Error::conflict("confirmed favorite has no boolean value"))?;
            let favorited_at = payload.get("favorited_at").and_then(Value::as_i64);
            tx.execute(
                "UPDATE songs SET is_favorite=?2, favorited_at=?3, updated_at=?4 WHERE uuid=?1",
                params![song_uuid, i64::from(favorite), favorited_at, now_ms()],
            )
            .map_err(storage)?;
        }
        "override" => {
            project_override(tx, song_uuid, object_uuid, payload)?;
        }
        "play-stats" => {
            project_play_stats(tx, song_uuid, song_exists, payload)?;
        }
        _ => unreachable!("object type was checked above"),
    }
    Ok(())
}

fn project_play_stats(
    tx: &Connection,
    song_uuid: &str,
    song_exists: bool,
    payload: &Value,
) -> Result<(), Error> {
    let counts = payload
        .get("by_device")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::conflict("confirmed play stats have no device buckets"))?;
    for (device, value) in counts {
        let device = uuid::Uuid::parse_str(device)
            .map_err(|_| Error::conflict("play stats contain an invalid device UUID"))?;
        let count = value
            .as_u64()
            .ok_or_else(|| Error::conflict("play stats contain an invalid count"))?;
        tx.execute(
            "INSERT INTO sync_device_play_counts(song_uuid, device_id, play_count) VALUES(?1, ?2, ?3) ON CONFLICT(song_uuid, device_id) DO UPDATE SET play_count=MAX(play_count, excluded.play_count)",
            params![song_uuid, device.to_string(), i64::try_from(count).unwrap_or(i64::MAX)],
        )
        .map_err(storage)?;
    }
    let mut bucket_statement = tx
        .prepare("SELECT play_count FROM sync_device_play_counts WHERE song_uuid=?1")
        .map_err(storage)?;
    let bucket_rows = bucket_statement
        .query_map(params![song_uuid], |row| row.get::<_, i64>(0))
        .map_err(storage)?;
    let mut total = 0i64;
    for row in bucket_rows {
        total = total.saturating_add(row.map_err(storage)?);
    }
    drop(bucket_statement);
    if song_exists {
        tx.execute(
            "UPDATE songs SET play_count=?2, updated_at=?3 WHERE uuid=?1",
            params![song_uuid, total, now_ms()],
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn project_override(
    tx: &Connection,
    song_uuid: &str,
    object_uuid: &str,
    payload: &Value,
) -> Result<(), Error> {
    let record: crate::domain::library::OverrideRecord = serde_json::from_value(payload.clone())
        .map_err(|_| Error::conflict("confirmed override has an invalid shape"))?;
    if record.song_uuid.to_string() != object_uuid || song_uuid != object_uuid {
        return Err(Error::conflict(
            "confirmed override UUID does not match its object key",
        ));
    }
    let revision = i64::try_from(record.revision.as_u64()).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO song_overrides(song_uuid,title,artist,album,lyrics_text,version,updated_at,revision,hlc_wall_secs,hlc_counter) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(song_uuid) DO UPDATE SET title=excluded.title,artist=excluded.artist,album=excluded.album,lyrics_text=excluded.lyrics_text,version=excluded.version,updated_at=excluded.updated_at,revision=excluded.revision,hlc_wall_secs=excluded.hlc_wall_secs,hlc_counter=excluded.hlc_counter",
        params![
            object_uuid,
            record.title,
            record.artist,
            record.album,
            record.lyrics_text,
            revision,
            now_ms(),
            revision,
            i64::try_from(record.hlc.wall_secs).unwrap_or(i64::MAX),
            i64::from(record.hlc.counter),
        ],
    )
    .map_err(storage)?;
    Ok(())
}

pub(super) fn confirmed_play_stats_payload(
    tx: &Connection,
    song_uuid: &str,
) -> Result<Value, Error> {
    let mut statement = tx
        .prepare(
            "SELECT device_id, play_count FROM sync_device_play_counts WHERE song_uuid=?1 ORDER BY device_id",
        )
        .map_err(storage)?;
    let buckets = statement
        .query_map(params![song_uuid], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    let by_device: serde_json::Map<String, Value> = buckets
        .into_iter()
        .map(|(device, count)| (device, Value::from(u64::try_from(count).unwrap_or(0))))
        .collect();
    let local_device: String = tx
        .query_row(
            "SELECT value FROM device_state WHERE key='device_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let local_device = uuid::Uuid::parse_str(&local_device).map_err(storage)?;
    let by_device = by_device
        .into_iter()
        .map(|(device, count)| {
            let device = uuid::Uuid::parse_str(&device).map_err(storage)?;
            let count = count.as_u64().ok_or_else(|| Error::InvariantViolation {
                why: "play-count projection produced a non-integer bucket".to_owned(),
            })?;
            Ok((device, count))
        })
        .collect::<Result<_, Error>>()?;
    serde_json::to_value(PlayStatsRecord {
        song_uuid: uuid::Uuid::parse_str(song_uuid).map_err(storage)?,
        revision: Revision::INITIAL,
        updated_by_device_id: LibraryDeviceId::from_uuid(local_device),
        hlc: HybridLogicalClock::new(0, 0),
        by_device,
    })
    .map_err(storage)
}

fn project_song(
    tx: &Connection,
    object_uuid: &str,
    payload: &Value,
    local_root_uuid: Option<&str>,
    preserve_file_facts: bool,
) -> Result<(), Error> {
    let open_delete_intent: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_local_delete_intents WHERE object_type='song' AND object_uuid=?1 AND state='open')",
            params![object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if open_delete_intent {
        // The entire local object, including the old path and resource identity,
        // is part of the undo overlay until the user restores or finalizes it.
        return Ok(());
    }
    let root_uuid = local_root_uuid
        .ok_or_else(|| Error::conflict("song projection requires a bound local library root"))?;
    let song_uuid = payload
        .get("song_uuid")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict("confirmed song is missing song_uuid"))?;
    if song_uuid != object_uuid {
        return Err(Error::conflict(
            "confirmed song UUID does not match its object key",
        ));
    }
    let relative_path = payload
        .get("relative_path")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict("confirmed song is missing its relative path"))?;
    let mut path = crate::domain::ids::RelativeMediaPath::new(relative_path)
        .map_err(|_| Error::conflict("confirmed song has an invalid relative path"))?;
    let title = optional_string(payload, "title")?;
    let artist = optional_string(payload, "artist")?;
    let album = optional_string(payload, "album")?;
    let added_at = payload
        .get("added_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::conflict("confirmed song is missing added_at"))?;
    let duration_ms = optional_nonnegative(payload, "duration_ms")?;
    let file_size = optional_nonnegative(payload, "file_size")?;
    let bitrate_bps = optional_nonnegative(payload, "bitrate_bps")?;
    let sample_rate_hz = optional_nonnegative(payload, "sample_rate_hz")?;
    let channels = optional_nonnegative(payload, "channels")?;
    let bits_per_sample = optional_nonnegative(payload, "bits_per_sample")?;
    let hash = optional_string(payload, "blake3_hash")?;
    if hash
        .as_deref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(Error::conflict("confirmed song has an invalid BLAKE3 hash"));
    }
    let format = optional_string(payload, "format")?;
    if preserve_file_facts
        || local_projection_conflicts(tx, root_uuid, song_uuid, &path, hash.as_deref())?
    {
        // The current bytes remain usable until immutable resource publication
        // verifies and associates their replacement. That device-local safety
        // rule must not discard already-confirmed shared tags: publication only
        // changes file facts, so skipping these tags would leave them stale
        // forever even after the new audio arrives. Pending local mutations and
        // delete intents were excluded before reaching this branch.
        tx.execute(
            "UPDATE songs SET title=?2,artist=?3,album=?4,title_sort=?5,artist_sort=?6,album_sort=?7,duration_ms=?8,added_at=?9,updated_at=?10 WHERE uuid=?1 AND library_root_uuid=?11",
            params![song_uuid,title,artist,album,normalized_key(title.as_deref().unwrap_or("")),normalized_key(artist.as_deref().unwrap_or("")),normalized_key(album.as_deref().unwrap_or("")),duration_ms,i64::try_from(added_at).unwrap_or(i64::MAX),now_ms(),root_uuid],
        ).map_err(storage)?;
        replace_song_search(tx, song_uuid, title, artist, album)?;
        return project_waiting_song_state(tx, root_uuid, song_uuid);
    }
    let path_owner: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM songs WHERE library_root_uuid=?1 AND normalized_relative_path=?2 AND uuid<>?3)",
            params![root_uuid, path.identity_key(), song_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    // Independently populated libraries can contain different audio at the
    // same relative path. Reserve a distinct missing row for the host object;
    // resource publication later associates its verified immutable local file.
    // Skipping the row here would leave downloaded audio outside the catalog.
    if path_owner {
        path = reserve_pending_projection_path(tx, root_uuid, song_uuid)?;
    }
    let title_sort = normalized_key(title.as_deref().unwrap_or(""));
    let artist_sort = normalized_key(artist.as_deref().unwrap_or(""));
    let album_sort = normalized_key(album.as_deref().unwrap_or(""));
    let now = now_ms();
    tx.execute(
        "INSERT INTO songs(uuid, library_root_uuid, relative_path, normalized_relative_path, blake3_hash, file_size, file_mtime_ns, format, title, artist, album, title_sort, artist_sort, album_sort, duration_ms, is_favorite, play_count, added_at, availability, revision, bitrate_bps, sample_rate_hz, channels, bits_per_sample, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, 0, ?15, 'missing', 0, ?16, ?17, ?18, ?19, ?20, ?20)
         ON CONFLICT(uuid) DO UPDATE SET library_root_uuid=excluded.library_root_uuid, relative_path=excluded.relative_path, normalized_relative_path=excluded.normalized_relative_path, blake3_hash=excluded.blake3_hash, file_size=excluded.file_size, format=excluded.format, title=excluded.title, artist=excluded.artist, album=excluded.album, title_sort=excluded.title_sort, artist_sort=excluded.artist_sort, album_sort=excluded.album_sort, duration_ms=excluded.duration_ms, added_at=excluded.added_at, bitrate_bps=excluded.bitrate_bps, sample_rate_hz=excluded.sample_rate_hz, channels=excluded.channels, bits_per_sample=excluded.bits_per_sample, availability=CASE WHEN songs.availability='pending_delete' THEN 'pending_delete' WHEN songs.availability='available' AND songs.blake3_hash IS excluded.blake3_hash AND songs.relative_path=excluded.relative_path THEN 'available' ELSE 'missing' END, updated_at=excluded.updated_at",
        params![song_uuid, root_uuid, path.display(), path.identity_key(), hash, file_size, format, title, artist, album, title_sort, artist_sort, album_sort, duration_ms, i64::try_from(added_at).unwrap_or(i64::MAX), bitrate_bps, sample_rate_hz, channels, bits_per_sample, now],
    )
    .map_err(super::support::map_constraint)?;
    replace_song_search(tx, song_uuid, title, artist, album)?;
    project_waiting_song_state(tx, root_uuid, song_uuid)?;
    Ok(())
}

fn replace_song_search(
    tx: &Connection,
    song_uuid: &str,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM song_search WHERE song_uuid=?1",
        params![song_uuid],
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT INTO song_search(title, artist, album, song_uuid) VALUES(?1, ?2, ?3, ?4)",
        params![
            title.unwrap_or_default(),
            artist.unwrap_or_default(),
            album.unwrap_or_default(),
            song_uuid
        ],
    )
    .map_err(storage)?;
    Ok(())
}

fn project_waiting_song_state(
    tx: &Connection,
    root_uuid: &str,
    song_uuid: &str,
) -> Result<(), Error> {
    let mut statement = tx
        .prepare(
            "SELECT object_type, object_uuid, payload_json FROM sync_object_versions WHERE is_tombstone=0 AND payload_json IS NOT NULL AND ((object_type IN ('favorite', 'override', 'play-stats') AND object_uuid=?1) OR object_type='playlist')",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(params![song_uuid], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    for (object_type, object_uuid, payload) in rows {
        let payload: Value = serde_json::from_str(&payload).map_err(storage)?;
        if object_type == "playlist" {
            let references_song = payload
                .get("members")
                .and_then(Value::as_array)
                .is_some_and(|members| {
                    members.iter().any(|member| {
                        member.get("song_uuid").and_then(Value::as_str) == Some(song_uuid)
                    })
                });
            if references_song {
                project_playlist(tx, &object_uuid, &payload, Some(root_uuid))?;
            }
        } else {
            project_confirmed_song_state(
                tx,
                &object_type,
                &object_uuid,
                &payload,
                Some(root_uuid),
            )?;
        }
    }
    Ok(())
}

fn project_playlist(
    tx: &Connection,
    object_uuid: &str,
    payload: &Value,
    local_root_uuid: Option<&str>,
) -> Result<(), Error> {
    let root_uuid = local_root_uuid.ok_or_else(|| {
        Error::conflict("playlist projection requires a bound local library root")
    })?;
    let playlist_uuid = payload
        .get("playlist_uuid")
        .and_then(Value::as_str)
        .unwrap_or(object_uuid);
    if playlist_uuid != object_uuid {
        return Err(Error::conflict(
            "confirmed playlist UUID does not match its object key",
        ));
    }
    let pending: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE object_type='playlist' AND object_uuid=?1 AND status='pending')",
            params![object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if pending {
        return Ok(());
    }
    let display_name = payload
        .get("display_name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict("confirmed playlist is missing its name"))?;
    if display_name.trim().is_empty() {
        return Err(Error::conflict("confirmed playlist name is empty"));
    }
    let created_at = payload
        .get("created_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::conflict("confirmed playlist is missing created_at"))?;
    let members = payload
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::conflict("confirmed playlist has no member snapshot"))?;
    let parsed = parse_playlist_members(members)?;
    let mut live_members = Vec::with_capacity(parsed.len());
    let mut missing_song = false;
    for (song_uuid, _, _, _) in &parsed {
        if is_tombstoned(tx, "song", song_uuid)? {
            continue;
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid=?2)",
                params![song_uuid, root_uuid],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !exists {
            missing_song = true;
            break;
        }
    }
    if missing_song {
        return Ok(());
    }
    for member in parsed {
        if !is_tombstoned(tx, "song", &member.0)? && !is_tombstoned(tx, "playlist-item", &member.1)?
        {
            live_members.push(member);
        }
    }
    let display_name = available_playlist_name(tx, root_uuid, object_uuid, display_name)?;
    let name_key = playlist_name_key(&display_name);
    tx.execute(
        "INSERT INTO playlists(uuid, library_root_uuid, display_name, normalized_name_key, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(uuid) DO UPDATE SET library_root_uuid=excluded.library_root_uuid, display_name=excluded.display_name, normalized_name_key=excluded.normalized_name_key, created_at=excluded.created_at, updated_at=excluded.updated_at",
        params![object_uuid, root_uuid, display_name, name_key, created_at, now_ms()],
    )
    .map_err(super::support::map_constraint)?;
    tx.execute(
        "DELETE FROM playlist_songs WHERE playlist_uuid=?1",
        params![object_uuid],
    )
    .map_err(storage)?;
    for (song_uuid, member_uuid, position, added_at) in live_members {
        tx.execute(
            "INSERT INTO playlist_songs(playlist_uuid, song_uuid, position, added_at, member_uuid) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![object_uuid, song_uuid, position, added_at, member_uuid],
        )
        .map_err(super::support::map_constraint)?;
    }
    Ok(())
}

fn available_playlist_name(
    tx: &Connection,
    root: &str,
    id: &str,
    requested: &str,
) -> Result<String, Error> {
    let mut name = requested.to_owned();
    let short_id: String = id.chars().take(8).collect();
    let mut attempt = 1_u64;
    loop {
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE library_root_uuid=?1 AND normalized_name_key=?2 AND uuid<>?3)",
            params![root, playlist_name_key(&name), id], |row| row.get(0),
        ).map_err(storage)?;
        if !occupied {
            return Ok(name);
        }
        // Different UUIDs are different lists even when their names normalize
        // equally. Keep the local owner and give the incoming list a stable,
        // readable suffix while respecting the 40-grapheme editing limit.
        let suffix = if attempt == 1 {
            format!(" ({short_id})")
        } else {
            format!(" ({short_id}-{attempt})")
        };
        let stem: String = requested
            .trim()
            .graphemes(true)
            .take(40 - suffix.len())
            .collect();
        name = format!("{}{suffix}", stem.trim_end());
        attempt = attempt
            .checked_add(1)
            .ok_or_else(|| Error::conflict("no vacant playlist display name"))?;
    }
}

type ProjectedPlaylistMember = (String, String, i64, i64);

fn parse_playlist_members(members: &[Value]) -> Result<Vec<ProjectedPlaylistMember>, Error> {
    let mut parsed = Vec::with_capacity(members.len());
    let mut member_ids = std::collections::BTreeSet::new();
    let mut song_ids = std::collections::BTreeSet::new();
    for member in members {
        let song_uuid = member
            .get("song_uuid")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::conflict("playlist member has no song UUID"))?;
        let member_uuid = member
            .get("member_uuid")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::conflict("playlist member has no stable member UUID"))?;
        uuid::Uuid::parse_str(song_uuid)
            .map_err(|_| Error::conflict("playlist member has invalid song UUID"))?;
        uuid::Uuid::parse_str(member_uuid)
            .map_err(|_| Error::conflict("playlist member has invalid member UUID"))?;
        let position = member
            .get("position")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| Error::conflict("playlist member position is invalid"))?;
        let added_at = member
            .get("added_at")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::conflict("playlist member added_at is invalid"))?;
        if !member_ids.insert(member_uuid.to_owned()) || !song_ids.insert(song_uuid.to_owned()) {
            return Err(Error::conflict(
                "playlist contains duplicate member identities",
            ));
        }
        parsed.push((
            song_uuid.to_owned(),
            member_uuid.to_owned(),
            position,
            added_at,
        ));
    }
    Ok(parsed)
}

fn optional_string(payload: &Value, key: &str) -> Result<Option<String>, Error> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(Error::conflict(format!(
            "confirmed song field {key} is invalid"
        ))),
    }
}

fn optional_nonnegative(payload: &Value, key: &str) -> Result<Option<i64>, Error> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|number| Some(i64::try_from(number).unwrap_or(i64::MAX)))
            .ok_or_else(|| Error::conflict(format!("confirmed song field {key} is invalid"))),
    }
}

/// Reserve a destination for a remote object whose own relative path is already
/// owned by a different local song. The name is derived from the object
/// identity, so replaying the same commit lands on the same row instead of
/// accumulating names, and it stays outside the media tree so the scanner never
/// treats it as a local file.
fn reserve_pending_projection_path(
    tx: &Connection,
    root_uuid: &str,
    song_uuid: &str,
) -> Result<crate::domain::ids::RelativeMediaPath, Error> {
    let mut suffix = 0_u64;
    loop {
        let candidate = crate::domain::ids::RelativeMediaPath::new(&format!(
            "echo/media/pending/{song_uuid}-{suffix}"
        ))?;
        let occupied: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE library_root_uuid=?1 AND normalized_relative_path=?2 AND uuid<>?3)",
                params![root_uuid, candidate.identity_key(), song_uuid],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !occupied {
            return Ok(candidate);
        }
        suffix = suffix
            .checked_add(1)
            .ok_or_else(|| Error::conflict("no vacant local song projection path"))?;
    }
}

/// Whether this device already holds file facts of the object that the
/// incoming one must not overwrite. Relative paths are device-local facts when
/// two independently populated libraries are merged: replacing a local path
/// with the peer's path makes a perfectly valid local file appear missing on
/// this device. Confirmed shared metadata is independent of these file facts.
fn local_projection_conflicts(
    tx: &Connection,
    root_uuid: &str,
    song_uuid: &str,
    path: &crate::domain::ids::RelativeMediaPath,
    hash: Option<&str>,
) -> Result<bool, Error> {
    let local_song: Option<(String, Option<String>, String)> = tx
        .query_row(
            "SELECT normalized_relative_path, blake3_hash, availability FROM songs WHERE uuid=?1 AND library_root_uuid=?2",
            params![song_uuid, root_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)?;
    let Some((local_path, local_hash, availability)) = local_song else {
        return Ok(false);
    };
    let has_pending_local_change: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE object_type='song' AND object_uuid=?1 AND local_root_uuid=?2 AND status='pending')",
            params![song_uuid, root_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    Ok(has_pending_local_change
        || (availability != "missing"
            && (local_path != path.identity_key() || local_hash.as_deref() != hash)))
}

#[cfg(test)]
mod tests;
