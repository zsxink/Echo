//! Capture the actual append snapshot and member after-images inside planning SQL.
use super::{
    ChangeOrigin, Connection, ControlPlanePort, DeviceId, Error, HybridLogicalClock, LibraryBatch,
    LibraryId, LibraryRootId, ObjectKey, PlaylistId, PortableRecord, PortableRecordEnvelope,
    RecordKind, Revision, SongId, planned_playlist, statements, storage,
};
use crate::domain::ids::PlaylistItemId;
use crate::domain::library::PlaylistItemRecord;
use rusqlite::params;

pub(super) struct Append {
    pub root: LibraryRootId,
    pub song: SongId,
    pub targets: Vec<PlaylistId>,
    pub device: DeviceId,
    pub library: LibraryId,
    pub sequence: u64,
}

impl Append {
    pub fn plan(
        self,
        c: &Connection,
        authority: &dyn ControlPlanePort,
    ) -> Result<LibraryBatch, Error> {
        let playable: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid=?2 AND availability='available')",
            params![self.song.to_string(), self.root.to_string()], |row| row.get(0),
        ).map_err(storage)?;
        if !playable {
            return Err(Error::unavailable("song", "not playable in active library"));
        }
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            self.library,
            ChangeOrigin::LocalEdit,
            vec![],
        );
        let mut targets = self.targets;
        targets.sort_unstable();
        targets.dedup();
        for playlist in targets {
            let belongs: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)",
                    params![playlist.to_string(), self.root.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if !belongs {
                return Err(Error::unavailable("playlist", "not in active library"));
            }
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlist_songs WHERE playlist_uuid=?1 AND song_uuid=?2)",
                params![playlist.to_string(), self.song.to_string()], |row| row.get(0),
            ).map_err(storage)?;
            if exists {
                continue;
            }
            let id = PlaylistItemId::new();
            statements::add_member(c, playlist, self.song, u64::MAX, id)?;
            let saved =
                authority.read_record(self.root, RecordKind::Playlist, &playlist.to_string())?;
            let playlist_record = planned_playlist(c, playlist, self.device, saved.as_ref())?;
            let (position, added_at, revision) = c.query_row(
                "SELECT position,added_at,member_revision FROM playlist_songs WHERE member_uuid=?1",
                [id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, u64>(2)?)),
            ).map_err(storage)?;
            let member = PortableRecord::PlaylistItem(PlaylistItemRecord {
                item_uuid: id.as_uuid(),
                playlist_uuid: playlist.as_uuid(),
                song_uuid: self.song.as_uuid(),
                position,
                added_at: Some(added_at),
                revision: Revision::from_u64(revision),
                hlc: crate::domain::library::next_hlc(
                    HybridLogicalClock::default(),
                    u64::try_from(super::super::now_ms().max(0)).unwrap_or(0) / 1000,
                ),
                updated_by_device_id: self.device,
            });
            batch
                .records
                .push(PortableRecordEnvelope::new(playlist_record, None)?);
            batch
                .records
                .push(PortableRecordEnvelope::new(member, None)?);
            batch.state.extend(
                super::super::sync::durability::capture_local_object_mutation(
                    c,
                    self.root,
                    ObjectKey::Playlist(playlist),
                    self.library,
                    crate::domain::sync::DeviceId::from_uuid(self.device.as_uuid()),
                    self.sequence,
                )?,
            );
        }
        Ok(batch)
    }
}
