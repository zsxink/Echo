//! Resolve only explicit root-scoped parents and identities; never invent them.
use super::{Reason, storage};
use crate::domain::{
    ids::LibraryRootId,
    library::{PortableRecord, PortableRecordEnvelope, RecordKind},
};
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension, params};

fn tombstoned(records: &[PortableRecordEnvelope], kind: RecordKind, id: uuid::Uuid) -> bool {
    records.iter().any(|e| matches!(e.record(), PortableRecord::Tombstone(t) if t.deleted_kind==kind && t.object_uuid==id))
}
fn parent(
    c: &Connection,
    root: LibraryRootId,
    id: uuid::Uuid,
    playlist: bool,
    records: &[PortableRecordEnvelope],
) -> Result<Option<Reason>, Error> {
    let kind = if playlist {
        RecordKind::Playlist
    } else {
        RecordKind::Song
    };
    if tombstoned(records, kind, id) {
        return Ok(Some(Reason::TombstonedDependency));
    }
    let sql = if playlist {
        "SELECT library_root_uuid FROM playlists WHERE uuid=?1"
    } else {
        "SELECT library_root_uuid FROM songs WHERE uuid=?1 AND availability<>'pending_delete'"
    };
    let owner: Option<String> = c
        .query_row(sql, [id.to_string()], |r| r.get(0))
        .optional()
        .map_err(storage)?;
    Ok(match owner {
        None => Some(if playlist {
            Reason::MissingPlaylist
        } else {
            Reason::MissingSong
        }),
        Some(owner) if owner != root.to_string() => Some(Reason::IdentityConflict),
        _ => None,
    })
}
pub(super) fn song(
    c: &Connection,
    root: LibraryRootId,
    id: uuid::Uuid,
    records: &[PortableRecordEnvelope],
) -> Result<Option<Reason>, Error> {
    parent(c, root, id, false, records)
}
pub(super) fn playlist(
    c: &Connection,
    root: LibraryRootId,
    id: uuid::Uuid,
    records: &[PortableRecordEnvelope],
) -> Result<Option<Reason>, Error> {
    parent(c, root, id, true, records)
}
fn conflict(
    c: &Connection,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<Option<Reason>, Error> {
    let exists: bool = c
        .query_row(sql, parameters, |r| r.get(0))
        .map_err(storage)?;
    Ok(exists.then_some(Reason::IdentityConflict))
}
pub(super) fn unresolved(
    c: &Connection,
    root: LibraryRootId,
    envelope: &PortableRecordEnvelope,
    records: &[PortableRecordEnvelope],
) -> Result<Option<Reason>, Error> {
    let root_text = root.to_string();
    let id = envelope.record().object_uuid().to_string();
    match envelope.record() {
        PortableRecord::Song(s) => {
            let path = crate::domain::ids::RelativeMediaPath::new(s.media_path.as_str())?;
            conflict(
                c,
                "SELECT EXISTS(SELECT 1 FROM songs WHERE (uuid=?1 AND library_root_uuid<>?2) OR (library_root_uuid=?2 AND normalized_relative_path=?3 AND uuid<>?1))",
                params![id, root_text, path.identity_key()],
            )
        }
        PortableRecord::Playlist(p) => conflict(
            c,
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE (uuid=?1 AND library_root_uuid<>?2) OR (library_root_uuid=?2 AND normalized_name_key=?3 AND uuid<>?1))",
            params![
                id,
                root_text,
                crate::domain::text::playlist_name_key(&p.display_name)
            ],
        ),
        PortableRecord::Favorite(s) => song(c, root, s.song_uuid, records),
        PortableRecord::PlayStats(s) => song(c, root, s.song_uuid, records),
        PortableRecord::Override(s) => song(c, root, s.song_uuid, records),
        PortableRecord::PlaylistItem(p) => {
            if let Some(reason) = parent(c, root, p.playlist_uuid, true, records)? {
                return Ok(Some(reason));
            }
            if let Some(reason) = song(c, root, p.song_uuid, records)? {
                return Ok(Some(reason));
            }
            conflict(
                c,
                "SELECT EXISTS(SELECT 1 FROM playlist_songs WHERE (member_uuid=?1 AND (playlist_uuid<>?2 OR song_uuid<>?3)) OR (playlist_uuid=?2 AND (song_uuid=?3 OR position=?4) AND member_uuid<>?1))",
                params![
                    id,
                    p.playlist_uuid.to_string(),
                    p.song_uuid.to_string(),
                    p.position
                ],
            )
        }
        PortableRecord::Tombstone(t) => match t.deleted_kind {
            RecordKind::Playlist => conflict(
                c,
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid<>?2)",
                params![id, root_text],
            ),
            RecordKind::PlaylistItem => conflict(
                c,
                "SELECT EXISTS(SELECT 1 FROM playlist_songs ps JOIN playlists p ON p.uuid=ps.playlist_uuid WHERE ps.member_uuid=?1 AND p.library_root_uuid<>?2)",
                params![id, root_text],
            ),
            _ => conflict(
                c,
                "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid<>?2)",
                params![id, root_text],
            ),
        },
    }
}
