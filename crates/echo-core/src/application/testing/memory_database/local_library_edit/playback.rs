//! Serialized fake recording preserves directory event identity across retries.
use super::{
    candidate, commit_then_project, project_records, ChangeOrigin, ControlPlanePort, DeviceId,
    Error, LibraryBatch, LibraryRootId, MemoryDatabase, PortableRecord, PortableRecordEnvelope,
    RecordKind, Revision, SongId,
};
use crate::application::ports::PlaybackEditPort;
use crate::domain::ids::PlaybackSessionId;
use crate::domain::library::durability::{DurableStateChange, PlaybackEvent, StateKind};
use crate::domain::library::{next_hlc, HybridLogicalClock, PlayStatsRecord};

impl PlaybackEditPort for MemoryDatabase {
    fn record_library_playback(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        session: PlaybackSessionId,
        song: SongId,
        device: DeviceId,
        qualified_at: u64,
    ) -> Result<bool, Error> {
        let mut store = self.lock();
        if device != store.device_id {
            return Err(Error::conflict("playback author is not the local device"));
        }
        let durable = control
            .durability()
            .ok_or_else(|| Error::unavailable("playback", "directory adapter missing"))?;
        let replay = durable.recover(root)?;
        let manifest = control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("playback", "manifest missing"))?;
        let mut planned = store.clone();
        project_records(&mut planned, root, &replay);
        if let Some(saved) = replay
            .states()
            .find(|state| state.kind == StateKind::PlaybackEvent && state.id == session.as_uuid())
        {
            let event: PlaybackEvent = serde_json::from_value(saved.payload.clone())
                .map_err(|error| Error::unavailable("playback event", error.to_string()))?;
            if event.song_uuid != song.as_uuid() {
                return Err(Error::conflict("playback session belongs to another song"));
            }
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::saved_pending_projection(
                    crate::domain::ids::OperationId::from_uuid(session.as_uuid()),
                    Error::unavailable("test transaction", "simulated commit failure"),
                ));
            }
            *store = planned;
            return Ok(false);
        }
        if !planned
            .songs
            .get(&song)
            .is_some_and(|current| current.root() == root && current.availability().is_visible())
        {
            return Ok(false);
        }
        let mut stats = match control.read_record(root, RecordKind::PlayStats, &song.to_string())? {
            Some(PortableRecord::PlayStats(stats)) => stats,
            _ => PlayStatsRecord {
                song_uuid: song.as_uuid(),
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc: HybridLogicalClock::default(),
                by_device: std::collections::BTreeMap::default(),
            },
        };
        let now = qualified_at;
        let count = stats.by_device.entry(device.as_uuid()).or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| Error::conflict("play count exhausted"))?;
        stats.revision = Revision::from_u64(
            stats
                .revision
                .as_u64()
                .checked_add(1)
                .ok_or_else(|| Error::conflict("playback revision exhausted"))?,
        );
        stats.hlc = next_hlc(stats.hlc, now / 1000);
        stats.updated_by_device_id = device;
        let record = PortableRecord::PlayStats(stats);
        let mut batch = LibraryBatch::new(
            session.as_uuid(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![PortableRecordEnvelope::new(record.clone(), None)?],
        );
        batch.state = candidate::freeze(
            &replay,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
        )?;
        batch.state.push(DurableStateChange::new(
            StateKind::PlaybackEvent,
            session.as_uuid(),
            serde_json::json!({"song_uuid":song.as_uuid(),"recorded_at":now}),
        ));
        commit_then_project(control, root, &batch, |_| {
            let replay = durable.recover(root)?;
            let mut planned = store.clone();
            project_records(&mut planned, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::unavailable(
                    "test transaction",
                    "simulated commit failure",
                ));
            }
            *store = planned;
            Ok(())
        })?;
        Ok(true)
    }
}
