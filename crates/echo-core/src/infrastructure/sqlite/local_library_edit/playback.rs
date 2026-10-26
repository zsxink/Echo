//! Freeze a private listen event and the actual shared bucket candidate together.
use super::{
    planned_batch, storage, ChangeOrigin, ControlPlanePort, DeviceId, Error, LibraryBatch,
    LibraryRootId, ObjectKey, PortableRecord, RecordKind, Revision, SongId, SqliteDatabase,
};
use crate::application::ports::PlaybackEditPort;
use crate::domain::ids::PlaybackSessionId;
use crate::domain::library::durability::{DurableStateChange, PlaybackEvent, StateKind};
use crate::domain::library::{next_hlc, HybridLogicalClock, PlayStatsRecord};
use rusqlite::{params, OptionalExtension};

impl PlaybackEditPort for SqliteDatabase {
    fn record_library_playback(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        session: PlaybackSessionId,
        song: SongId,
        device: DeviceId,
        qualified_at: u64,
    ) -> Result<bool, Error> {
        self.plan_library_change(control, root, device, Some(session.as_uuid()), move |c, authority, library, sequence, _| {
            let empty = || LibraryBatch::new(session.as_uuid(), library, ChangeOrigin::LocalEdit, vec![]);
            let saved: Option<String> = c.query_row(
                "SELECT song_uuid FROM recorded_play_sessions WHERE playback_session_uuid=?1",
                [session.to_string()], |row| row.get(0),
            ).optional().map_err(storage)?;
            if let Some(saved) = saved {
                if saved != song.to_string() { return Err(Error::conflict("playback session belongs to another song")); }
                return Ok((empty(), false));
            }
            let visible: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid=?2 AND availability<>'pending_delete')",
                params![song.to_string(), root.to_string()], |row| row.get(0),
            ).map_err(storage)?;
            if !visible { return Ok((empty(), false)); }
            let before = authority.read_record(root, RecordKind::PlayStats, &song.to_string())?;
            // Use the caller's saved intent time, never the retry clock: a
            // prepared-but-unsaved operation must replan to the same payload.
            let now = qualified_at;
            let mut counts = super::super::read_device_play_counts(c, song)?;
            let count = counts.entry(device.as_uuid()).or_default();
            *count = count.checked_add(1).ok_or_else(|| Error::conflict("play count exhausted"))?;
            let revision = super::super::sync::current_outbox_revision(c, "play-stats", &song.to_string())?
                .checked_add(1).ok_or_else(|| Error::conflict("playback revision exhausted"))?;
            let stats = PlayStatsRecord {
                song_uuid: song.as_uuid(), revision: Revision::from_u64(u64::try_from(revision).map_err(storage)?),
                updated_by_device_id: device,
                hlc: next_hlc(before.as_ref().map_or_else(HybridLogicalClock::default, PortableRecord::hlc), now / 1000),
                by_device: counts,
            };
            // A rebuilt library can retain another author's unconfirmed bucket.
            // Preserve it in the local record, but only the authenticated author
            // may increase a bucket in a new event. The host merges omitted
            // confirmed buckets by max; it must never see A's pending delta as
            // a claim made by a new author B.
            let mut upload = stats.clone();
            upload.by_device.retain(|author, _| *author == device.as_uuid());
            super::statements::enqueue_object_snapshot_with_id(
                c,
                "play-stats",
                &song.to_string(),
                &serde_json::to_value(&upload).map_err(storage)?,
                "upsert",
                Some(
                    &super::super::sync::durability::identity(
                        "playback-mutation",
                        &session.to_string(),
                    )
                    .to_string(),
                ),
                Some(now),
            )?;
            let mut batch = planned_batch(c, root, library, device, PortableRecord::PlayStats(stats), ObjectKey::PlayStats(song), sequence)?;
            batch.operation_id = session.as_uuid();
            batch.state.push(DurableStateChange::new(StateKind::PlaybackEvent, session.as_uuid(),
                serde_json::to_value(PlaybackEvent { song_uuid: song.as_uuid(), recorded_at: now }).map_err(storage)?));
            Ok((batch, true))
        })
    }
}
