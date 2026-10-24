//! Freeze a stable member deletion and the remaining playlist snapshot together.
use super::{
    planned_batch, planned_playlist, statements, storage, ChangeOrigin, Connection,
    ControlPlanePort, DeviceId, Error, HybridLogicalClock, LibraryBatch, LibraryId, LibraryRootId,
    ObjectKey, PlaylistId, PortableRecord, PortableRecordEnvelope, RecordKind, Revision, SongId,
};
use crate::domain::ids::PlaylistItemId;
use crate::domain::library::{next_hlc, TombstoneRecord};
use rusqlite::{params, OptionalExtension};

pub(super) struct Removal {
    pub root: LibraryRootId,
    pub playlist: PlaylistId,
    pub song: SongId,
    pub device: DeviceId,
    pub library: LibraryId,
    pub sequence: u64,
}

impl Removal {
    pub fn plan(
        self,
        c: &Connection,
        authority: &dyn ControlPlanePort,
    ) -> Result<LibraryBatch, Error> {
        let belongs: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)",
                params![self.playlist.to_string(), self.root.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !belongs {
            return Err(Error::unavailable("playlist", "not in active library"));
        }
        let member: Option<String> = c
            .query_row(
                "SELECT member_uuid FROM playlist_songs WHERE playlist_uuid=?1 AND song_uuid=?2",
                params![self.playlist.to_string(), self.song.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(member) = member else {
            return Ok(LibraryBatch::new(
                uuid::Uuid::new_v4(),
                self.library,
                ChangeOrigin::LocalEdit,
                vec![],
            ));
        };
        let member = PlaylistItemId::from_uuid(uuid::Uuid::parse_str(&member).map_err(storage)?);
        let object = ObjectKey::PlaylistItem(member);
        let before =
            authority.read_record(self.root, RecordKind::PlaylistItem, &member.to_string())?;
        let revision = super::super::sync::durability::allocate_local_delete(c, self.root, object)?;
        let now = u64::try_from(super::super::now_ms().max(0)).unwrap_or(0) / 1000;
        let record = PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: member.as_uuid(),
            deleted_kind: RecordKind::PlaylistItem,
            revision: Revision::from_u64(revision),
            hlc: next_hlc(
                before
                    .as_ref()
                    .map_or_else(HybridLogicalClock::default, PortableRecord::hlc),
                now,
            ),
            updated_by_device_id: self.device,
        });
        let mut batch = planned_batch(
            c,
            self.root,
            self.library,
            self.device,
            record,
            object,
            self.sequence,
        )?;
        c.execute(
            "DELETE FROM playlist_songs WHERE member_uuid=?1",
            [member.to_string()],
        )
        .map_err(storage)?;
        statements::enqueue_object_snapshot(
            c,
            "playlist",
            &self.playlist.to_string(),
            &serde_json::json!({}),
            "upsert",
        )?;
        statements::advance_object_hlc(c, "playlist", &self.playlist.to_string(), now)?;
        let saved =
            authority.read_record(self.root, RecordKind::Playlist, &self.playlist.to_string())?;
        let record = planned_playlist(c, self.playlist, self.device, saved.as_ref())?;
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
        batch.state.extend(
            super::super::sync::durability::capture_local_object_mutation(
                c,
                self.root,
                ObjectKey::Playlist(self.playlist),
                self.library,
                crate::domain::sync::DeviceId::from_uuid(self.device.as_uuid()),
                self.sequence,
            )?,
        );
        Ok(batch)
    }
}
