//! Immutable song identities and canonicalization at the host transaction boundary.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::support::storage;
use crate::domain::ids::SongId;
use crate::domain::sync::{MutationCandidate, ObjectKey};
use crate::error::Error;

pub(super) fn resolve(connection: &Connection, id: &str) -> Result<String, Error> {
    Ok(connection
        .query_row(
            "SELECT canonical_song_uuid FROM sync_song_aliases WHERE alias_song_uuid=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .unwrap_or_else(|| id.to_owned()))
}

fn remap_payload(connection: &Connection, payload: &mut Value) -> Result<(), Error> {
    if let Some(id) = payload.get_mut("song_uuid") {
        if let Some(original) = id.as_str() {
            *id = Value::String(resolve(connection, original)?);
        }
    }
    if let Some(members) = payload.get_mut("members").and_then(Value::as_array_mut) {
        for member in members {
            remap_payload(connection, member)?;
        }
    }
    Ok(())
}

/// The returned candidate is an ephemeral canonical view. The original queued
/// mutation and inbox digest are never rewritten, including after ACK loss.
pub(super) fn normalize(
    connection: &Connection,
    mut candidate: MutationCandidate,
) -> Result<(MutationCandidate, Option<(String, String)>), Error> {
    let mut alias = None;
    let original = match candidate.object {
        ObjectKey::Song(id)
        | ObjectKey::Favorite(id)
        | ObjectKey::Override(id)
        | ObjectKey::PlayStats(id) => Some(id.to_string()),
        _ => None,
    };
    if let Some(original) = original {
        let mut canonical = resolve(connection, &original)?;
        if canonical == original
            && matches!(candidate.object, ObjectKey::Song(_))
            && candidate.payload != serde_json::json!({"tombstone":true})
        {
            if let Some(hash) = candidate
                .payload
                .get("blake3_hash")
                .and_then(Value::as_str)
                .filter(|hash| !hash.is_empty())
            {
                let first: Option<String> = connection.query_row(
                    "SELECT v.object_uuid FROM sync_object_versions v WHERE v.object_type='song' AND v.is_tombstone=0 AND json_extract(v.payload_json,'$.blake3_hash')=?1 AND v.object_uuid<>?2 AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=v.object_uuid) ORDER BY v.commit_seq,v.object_uuid LIMIT 1", params![hash,original], |row| row.get(0)).optional().map_err(storage)?;
                if let Some(first) = first {
                    // A UUID which was finally deleted cannot become an alias
                    // to a later reimport with the same bytes.
                    if !super::sync_projection::is_tombstoned(connection, "song", &original)? {
                        canonical = first;
                        alias = Some((original, canonical.clone()));
                        let payload: String = connection.query_row("SELECT payload_json FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1",params![canonical],|row|row.get(0)).map_err(storage)?;
                        candidate.payload = serde_json::from_str(&payload).map_err(storage)?;
                        candidate.resource_refs =
                            super::sync_manifests::load(connection, "song", &canonical)?;
                    }
                }
            }
        }
        let id = SongId::from_uuid(uuid::Uuid::parse_str(&canonical).map_err(storage)?);
        candidate.object = match candidate.object {
            ObjectKey::Song(_) => ObjectKey::Song(id),
            ObjectKey::Favorite(_) => ObjectKey::Favorite(id),
            ObjectKey::Override(_) => ObjectKey::Override(id),
            ObjectKey::PlayStats(_) => ObjectKey::PlayStats(id),
            other => other,
        };
        if candidate.payload.get("song_uuid").is_some() {
            candidate.payload["song_uuid"] = Value::String(canonical);
        }
    }
    if matches!(candidate.object, ObjectKey::Song(_))
        && candidate.payload != serde_json::json!({"tombstone":true})
    {
        preserve_import_time(connection, &mut candidate)?;
        if alias.is_none() {
            avoid_path_collision(connection, &mut candidate)?;
        }
    }
    remap_payload(connection, &mut candidate.payload)?;
    Ok((candidate, alias))
}

pub(super) fn persist(
    connection: &Connection,
    alias: &str,
    canonical: &str,
    sequence: i64,
) -> Result<(), Error> {
    uuid::Uuid::parse_str(alias).map_err(storage)?;
    uuid::Uuid::parse_str(canonical).map_err(storage)?;
    if alias == canonical || resolve(connection, canonical)? != canonical {
        return Err(Error::conflict(
            "song alias cannot target itself or another alias",
        ));
    }
    let previous = resolve(connection, alias)?;
    if previous != alias && previous != canonical {
        return Err(Error::conflict("song alias identity was remapped"));
    }
    if super::sync_projection::is_tombstoned(connection, "song", alias)? {
        return Err(Error::conflict(
            "finally deleted song cannot become an alias",
        ));
    }
    // Capture the physical location before releasing the alias's path. The
    // canonical host path can be completely different on this device.
    let local_audio: Option<(String, String)> = connection.query_row(
        "SELECT s.relative_path,s.normalized_relative_path FROM songs s JOIN sync_object_versions v ON v.object_type='song' AND v.object_uuid=?2 AND v.is_tombstone=0 WHERE s.uuid=?1 AND s.availability='available' AND s.blake3_hash IS NOT NULL AND s.blake3_hash=json_extract(v.payload_json,'$.blake3_hash') AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents i WHERE i.object_type='song' AND i.object_uuid=s.uuid AND i.state='open')",
        params![alias, canonical], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(storage)?;
    connection.execute("INSERT OR IGNORE INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) VALUES(?1,?2,?3)",params![alias,canonical,sequence]).map_err(storage)?;
    // Keep the old row for immutable queued requests and operation/resource
    // recovery, but remove it from catalog views and release its path identity.
    connection.execute("UPDATE songs SET availability='pending_delete',relative_path=?2,normalized_relative_path=?2 WHERE uuid=?1",params![alias,format!(".echo-sync-alias/{alias}")]).map_err(storage)?;
    if let Some((path, normalized_path)) = local_audio {
        preserve_local_audio(connection, alias, canonical, &path, &normalized_path)?;
    }
    connection
        .execute("DELETE FROM song_search WHERE song_uuid=?1", params![alias])
        .map_err(storage)?;
    Ok(())
}

fn preserve_local_audio(
    connection: &Connection,
    alias: &str,
    canonical: &str,
    path: &str,
    normalized_path: &str,
) -> Result<(), Error> {
    // Only the read model changes identity: immutable host payloads, manifests,
    // queued requests and the alias row remain available for recovery. A local
    // playable canonical or an undoable deletion always keeps its own state.
    connection.execute(
        "INSERT INTO songs(uuid,library_root_uuid,relative_path,normalized_relative_path,blake3_hash,file_size,file_mtime_ns,format,title,artist,album,title_sort,artist_sort,album_sort,duration_ms,is_favorite,play_count,added_at,availability,revision,parse_error,created_at,updated_at,cover_hash,bitrate_bps,sample_rate_hz,channels,bits_per_sample,hlc_wall_secs,hlc_counter,favorited_at)
         SELECT ?2,library_root_uuid,?3,?4,blake3_hash,file_size,file_mtime_ns,format,title,artist,album,title_sort,artist_sort,album_sort,duration_ms,is_favorite,play_count,added_at,'available',revision,parse_error,created_at,updated_at,cover_hash,bitrate_bps,sample_rate_hz,channels,bits_per_sample,hlc_wall_secs,hlc_counter,favorited_at FROM songs WHERE uuid=?1
         ON CONFLICT(uuid) DO UPDATE SET relative_path=excluded.relative_path,normalized_relative_path=excluded.normalized_relative_path,file_size=excluded.file_size,file_mtime_ns=excluded.file_mtime_ns,format=excluded.format,availability='available',cover_hash=COALESCE(songs.cover_hash,excluded.cover_hash),bitrate_bps=excluded.bitrate_bps,sample_rate_hz=excluded.sample_rate_hz,channels=excluded.channels,bits_per_sample=excluded.bits_per_sample
         WHERE songs.library_root_uuid=excluded.library_root_uuid AND songs.availability='missing' AND songs.blake3_hash=excluded.blake3_hash AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents WHERE object_type='song' AND object_uuid=?2 AND state='open')",
        params![alias, canonical, path, normalized_path],
    ).map_err(storage)?;
    connection.execute(
        "INSERT OR IGNORE INTO song_lyrics(song_uuid,source,text_kind,raw_text,timed_lines_json,source_mtime_ns,parse_error,updated_at) SELECT ?2,source,text_kind,raw_text,timed_lines_json,source_mtime_ns,parse_error,updated_at FROM song_lyrics WHERE song_uuid=?1",
        params![alias, canonical],
    ).map_err(storage)?;
    // The canonical may have been created from the local row while its song
    // mutation is still pending. Index it here because pending overlays can
    // intentionally defer the regular confirmed-song projector.
    connection
        .execute(
            "DELETE FROM song_search WHERE song_uuid=?1",
            params![canonical],
        )
        .map_err(storage)?;
    connection.execute("INSERT INTO song_search(title,artist,album,song_uuid) SELECT COALESCE(title,''),COALESCE(artist,''),COALESCE(album,''),uuid FROM songs WHERE uuid=?1",params![canonical]).map_err(storage)?;
    Ok(())
}

pub(super) fn project(connection: &Connection, alias: &str, canonical: &str) -> Result<(), Error> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1)",
            params![canonical],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !exists {
        return Ok(());
    }
    // Pending local state stays effective while its original events travel to
    // the host. Confirmed state subsequently projects using the canonical key.
    connection.execute("UPDATE songs SET is_favorite=CASE WHEN EXISTS(SELECT 1 FROM sync_mutations WHERE object_type='favorite' AND object_uuid=?1 AND status='pending') THEN COALESCE((SELECT is_favorite FROM songs WHERE uuid=?1),is_favorite) ELSE is_favorite END,favorited_at=CASE WHEN EXISTS(SELECT 1 FROM sync_mutations WHERE object_type='favorite' AND object_uuid=?1 AND status='pending') THEN (SELECT favorited_at FROM songs WHERE uuid=?1) ELSE favorited_at END WHERE uuid=?2",params![alias,canonical]).map_err(storage)?;
    // If both identities are present, preserve the original member UUID until
    // the host confirms the playlist merge and emits its typed member alias.
    connection
        .execute(
            "UPDATE playlist_songs SET song_uuid=?2 WHERE song_uuid=?1 AND playlist_uuid NOT IN (SELECT playlist_uuid FROM playlist_songs WHERE song_uuid=?2)",
            params![alias, canonical],
        )
        .map_err(storage)?;
    connection.execute("INSERT INTO sync_device_play_counts(song_uuid,device_id,play_count) SELECT ?2,device_id,play_count FROM sync_device_play_counts WHERE song_uuid=?1 ON CONFLICT(song_uuid,device_id) DO UPDATE SET play_count=MAX(play_count,excluded.play_count)",params![alias,canonical]).map_err(storage)?;
    connection.execute("UPDATE songs SET play_count=COALESCE((SELECT SUM(play_count) FROM sync_device_play_counts WHERE song_uuid=?1),play_count) WHERE uuid=?1",params![canonical]).map_err(storage)?;
    connection.execute("DELETE FROM song_overrides WHERE song_uuid=?2 AND EXISTS(SELECT 1 FROM sync_mutations WHERE object_type='override' AND object_uuid=?1 AND status='pending')",params![alias,canonical]).map_err(storage)?;
    connection.execute("INSERT OR IGNORE INTO song_overrides SELECT ?2,title,artist,album,lyrics_text,version,updated_at,revision,hlc_wall_secs,hlc_counter FROM song_overrides WHERE song_uuid=?1",params![alias,canonical]).map_err(storage)?;
    Ok(())
}

fn preserve_import_time(
    connection: &Connection,
    candidate: &mut MutationCandidate,
) -> Result<(), Error> {
    let (_, id) = super::sync_manifests::identity(candidate.object);
    let original: Option<String> = connection.query_row("SELECT payload_json FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1 AND is_tombstone=0",params![id],|row|row.get(0)).optional().map_err(storage)?;
    if let Some(original) = original {
        let original: Value = serde_json::from_str(&original).map_err(storage)?;
        if candidate
            .payload
            .get("relative_path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.starts_with(".echo-sync-alias/"))
        {
            candidate.payload["relative_path"] = original["relative_path"].clone();
        }
        if let Some(added_at) = original.get("added_at") {
            candidate.payload["added_at"] = added_at.clone();
        }
    }
    Ok(())
}

fn avoid_path_collision(
    connection: &Connection,
    candidate: &mut MutationCandidate,
) -> Result<(), Error> {
    let (_, id) = super::sync_manifests::identity(candidate.object);
    let Some(original) = candidate
        .payload
        .get("relative_path")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(());
    };
    let path = crate::domain::ids::RelativeMediaPath::new(&original)?;
    let taken: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM songs WHERE normalized_relative_path=?1 AND uuid<>?2 AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=songs.uuid))",params![path.identity_key(),id],|row|row.get(0)).map_err(storage)?;
    if !taken {
        return Ok(());
    }
    let (stem, extension) = original
        .rsplit_once('.')
        .map_or((original.as_str(), ""), |(stem, extension)| {
            (stem, extension)
        });
    let suffix = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };
    let new_path = format!("{stem} ({id}){suffix}");
    let new_path = crate::domain::ids::RelativeMediaPath::new(&new_path)?;
    let collision: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM songs WHERE normalized_relative_path=?1 AND uuid<>?2)",
            params![new_path.identity_key(), id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if collision {
        return Err(Error::conflict(
            "canonical import conflict path is already occupied",
        ));
    }
    candidate.payload["relative_path"] = Value::String(new_path.display().to_owned());
    for resource in &mut candidate.resource_refs {
        let resource_path = resource.relative_path.as_str();
        let renamed = if resource_path == original {
            Some(new_path.display().to_owned())
        } else {
            resource_path
                .strip_prefix(stem)
                .and_then(|tail| tail.strip_prefix('.'))
                .filter(|extension| !extension.contains('/'))
                .map(|extension| format!("{stem} ({id}).{extension}"))
        };
        if let Some(renamed) = renamed {
            *resource = crate::domain::sync::SharedResourceManifest::new(
                resource.kind,
                resource.digest,
                resource.length,
                resource.media_type.clone(),
                &renamed,
            )?;
        }
    }
    Ok(())
}
