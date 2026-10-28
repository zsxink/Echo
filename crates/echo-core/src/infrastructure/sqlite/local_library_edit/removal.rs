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
        replay: &crate::domain::library::durability::LibraryReplay,
    ) -> Result<LibraryBatch, Error> {
        let Some(member) = self.current_member(c)? else {
            return Ok(LibraryBatch::new(
                uuid::Uuid::new_v4(),
                self.library,
                ChangeOrigin::LocalEdit,
                vec![],
            ));
        };
        let object = ObjectKey::PlaylistItem(member);
        let started = std::time::Instant::now();
        let before = super::planned_record(
            authority,
            self.root,
            RecordKind::PlaylistItem,
            member.as_uuid(),
            replay,
        )?;
        super::trace_local_edit_stage(self.root, "membership_remove_read_member_record", started);
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
        let started = std::time::Instant::now();
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
        super::trace_local_edit_stage(self.root, "membership_remove_sql_delete", started);
        let started = std::time::Instant::now();
        let saved = super::planned_record(
            authority,
            self.root,
            RecordKind::Playlist,
            self.playlist.as_uuid(),
            replay,
        )?;
        super::trace_local_edit_stage(self.root, "membership_remove_read_playlist_record", started);
        let started = std::time::Instant::now();
        let record = planned_playlist(c, self.playlist, self.device, saved.as_ref())?;
        super::trace_local_edit_stage(self.root, "membership_remove_materialize_playlist", started);
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
        let started = std::time::Instant::now();
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
        super::trace_local_edit_stage(self.root, "membership_remove_capture_mutation", started);
        Ok(batch)
    }

    fn current_member(&self, c: &Connection) -> Result<Option<PlaylistItemId>, Error> {
        let started = std::time::Instant::now();
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
        super::trace_local_edit_stage(self.root, "membership_remove_preconditions", started);
        member
            .map(|member| {
                uuid::Uuid::parse_str(&member)
                    .map(PlaylistItemId::from_uuid)
                    .map_err(storage)
            })
            .transpose()
    }
}
