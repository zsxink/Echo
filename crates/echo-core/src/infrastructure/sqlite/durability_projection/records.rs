//! SQL-only materialization of resolved portable objects; no mutation/outbox helpers.
use super::super::support::storage;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::{PortableRecord, PortableRecordEnvelope};
use crate::domain::text::{normalized_key, playlist_name_key};
use crate::error::Error;
use rusqlite::{params, Connection};
pub(super) fn project_record(
    c: &Connection,
    root: LibraryRootId,
    envelope: &PortableRecordEnvelope,
) -> Result<(), Error> {
    let record = envelope.record();
    let id = record.object_uuid().to_string();
    match record {
        PortableRecord::Song(s) => {
            let path = crate::domain::ids::RelativeMediaPath::new(s.media_path.as_str())?;
            c.execute("INSERT INTO songs(uuid,library_root_uuid,relative_path,normalized_relative_path,blake3_hash,title,artist,album,title_sort,artist_sort,album_sort,added_at,availability,revision,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'missing',?13,?12,?12) ON CONFLICT(uuid) DO UPDATE SET relative_path=excluded.relative_path,normalized_relative_path=excluded.normalized_relative_path,blake3_hash=excluded.blake3_hash,title=excluded.title,artist=excluded.artist,album=excluded.album,title_sort=excluded.title_sort,artist_sort=excluded.artist_sort,album_sort=excluded.album_sort,added_at=excluded.added_at,revision=excluded.revision",params![id,root.to_string(),path.normalized(),path.identity_key(),s.content_hash,s.title,s.artist,s.album,normalized_key(s.title.as_deref().unwrap_or("")),normalized_key(s.artist.as_deref().unwrap_or("")),normalized_key(s.album.as_deref().unwrap_or("")),s.added_at,s.revision.as_u64()]).map_err(storage)?;
            c.execute("DELETE FROM song_search WHERE song_uuid=?1", [&id])
                .map_err(storage)?;
            c.execute(
                "INSERT INTO song_search(title,artist,album,song_uuid) VALUES(?1,?2,?3,?4)",
                params![
                    normalized_key(s.title.as_deref().unwrap_or("")),
                    normalized_key(s.artist.as_deref().unwrap_or("")),
                    normalized_key(s.album.as_deref().unwrap_or("")),
                    id
                ],
            )
            .map_err(storage)?;
        }
        PortableRecord::Favorite(s) => {
            c.execute(
                "UPDATE songs SET is_favorite=?2 WHERE uuid=?1 AND library_root_uuid=?3",
                params![id, s.is_favorite, root.to_string()],
            )
            .map_err(storage)?;
        }
        PortableRecord::Playlist(p) => {
            c.execute("INSERT INTO playlists(uuid,library_root_uuid,display_name,normalized_name_key,created_at,updated_at,revision) VALUES(?1,?2,?3,?4,?5,?5,?6) ON CONFLICT(uuid) DO UPDATE SET display_name=excluded.display_name,normalized_name_key=excluded.normalized_name_key,revision=excluded.revision,created_at=excluded.created_at",params![id,root.to_string(),p.display_name,playlist_name_key(&p.display_name),p.created_at.unwrap_or(0),p.revision.as_u64()]).map_err(storage)?;
        }
        PortableRecord::PlaylistItem(p) => {
            c.execute("INSERT INTO playlist_songs(playlist_uuid,song_uuid,position,added_at,member_uuid,member_revision) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(playlist_uuid,song_uuid) DO UPDATE SET position=excluded.position,added_at=excluded.added_at,member_uuid=excluded.member_uuid,member_revision=excluded.member_revision",params![p.playlist_uuid.to_string(),p.song_uuid.to_string(),p.position,p.added_at.unwrap_or(0),id,p.revision.as_u64()]).map_err(storage)?;
        }
        PortableRecord::PlayStats(s) => {
            for (device, count) in &s.by_device {
                c.execute("INSERT INTO sync_device_play_counts(song_uuid,device_id,play_count) VALUES(?1,?2,?3) ON CONFLICT(song_uuid,device_id) DO UPDATE SET play_count=MAX(play_count,excluded.play_count)",params![id,device.to_string(),i64::try_from(*count).unwrap_or(i64::MAX)]).map_err(storage)?;
            }
            c.execute(
                "UPDATE songs SET play_count=?2 WHERE uuid=?1 AND library_root_uuid=?3",
                params![id, total_count(c, &id)?, root.to_string()],
            )
            .map_err(storage)?;
        }
        PortableRecord::Override(o) => {
            c.execute("INSERT INTO song_overrides(song_uuid,title,artist,album,lyrics_text,revision,updated_at) VALUES(?1,?2,?3,?4,?5,?6,0) ON CONFLICT(song_uuid) DO UPDATE SET title=excluded.title,artist=excluded.artist,album=excluded.album,lyrics_text=excluded.lyrics_text,revision=excluded.revision",params![id,o.title,o.artist,o.album,o.lyrics_text,o.revision.as_u64()]).map_err(storage)?;
        }
        PortableRecord::Tombstone(t) => match t.deleted_kind {
            crate::domain::library::RecordKind::Song => {
                c.execute("UPDATE songs SET availability='pending_delete',is_favorite=0 WHERE uuid=?1 AND library_root_uuid=?2",params![id,root.to_string()]).map_err(storage)?;
                c.execute("DELETE FROM playlist_songs WHERE song_uuid=?1", [&id])
                    .map_err(storage)?;
            }
            crate::domain::library::RecordKind::Playlist => {
                c.execute(
                    "DELETE FROM playlists WHERE uuid=?1 AND library_root_uuid=?2",
                    params![id, root.to_string()],
                )
                .map_err(storage)?;
            }
            crate::domain::library::RecordKind::PlaylistItem => {
                c.execute("DELETE FROM playlist_songs WHERE member_uuid=?1", [&id])
                    .map_err(storage)?;
            }
            crate::domain::library::RecordKind::Favorite => {
                c.execute(
                    "UPDATE songs SET is_favorite=0 WHERE uuid=?1 AND library_root_uuid=?2",
                    params![id, root.to_string()],
                )
                .map_err(storage)?;
            }
            crate::domain::library::RecordKind::Override => {
                c.execute("DELETE FROM song_overrides WHERE song_uuid=?1", [&id])
                    .map_err(storage)?;
            }
            crate::domain::library::RecordKind::PlayStats => {
                c.execute(
                    "DELETE FROM sync_device_play_counts WHERE song_uuid=?1",
                    [&id],
                )
                .map_err(storage)?;
                c.execute(
                    "UPDATE songs SET play_count=0 WHERE uuid=?1 AND library_root_uuid=?2",
                    params![id, root.to_string()],
                )
                .map_err(storage)?;
            }
            crate::domain::library::RecordKind::Tombstone => {}
        },
    }
    Ok(())
}

fn total_count(c: &Connection, id: &str) -> Result<i64, Error> {
    let mut statement = c
        .prepare("SELECT play_count FROM sync_device_play_counts WHERE song_uuid=?1")
        .map_err(storage)?;
    let mut values = statement
        .query_map([id], |r| r.get::<_, i64>(0))
        .map_err(storage)?;
    values.try_fold(0i64, |total, value| {
        Ok(total.saturating_add(value.map_err(storage)?))
    })
}
