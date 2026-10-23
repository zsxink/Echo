//! Qualified playback events are committed to the music library before their
//! absolute per-device buckets are projected. Repeating a session cannot count twice.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::library_commit::commit_then_project;
use crate::application::ports::{ControlPlanePort, SongRepository};
use crate::domain::ids::{PlaybackSessionId, Revision, SongId};
use crate::domain::library::durability::{
    ChangeOrigin, DurableStateChange, LibraryBatch, PlaybackEvent, StateKind,
};
use crate::domain::library::{
    next_hlc, DeviceId, HybridLogicalClock, PlayStatsRecord, PortableRecord,
    PortableRecordEnvelope, RecordKind,
};
use crate::error::Error;

/// An absolute projection, never an increment after a portable commit.
pub trait PlaybackProjection: Send + Sync {
    /// Apply a saved event and its monotone buckets atomically and idempotently.
    ///
    /// # Errors
    /// Reports storage failures without discarding the saved playback event.
    fn project_playback(
        &self,
        session: PlaybackSessionId,
        song: SongId,
        stats: &PlayStatsRecord,
    ) -> Result<(), Error>;
}

/// Save one listen. Missing/hidden songs do not create playback facts.
/// # Errors
/// Library persistence failures and saved-pending-projection retain operation identity.
pub fn record_playback(
    control: &dyn ControlPlanePort,
    songs: &dyn SongRepository,
    projection: &dyn PlaybackProjection,
    device: DeviceId,
    session: PlaybackSessionId,
    song: SongId,
) -> Result<bool, Error> {
    let Some(current) = songs.by_id(song)? else {
        return Ok(false);
    };
    if !current.availability().is_visible() {
        return Ok(false);
    }
    let root = current.root();
    let manifest = control
        .read_manifest(root)?
        .ok_or_else(|| Error::unavailable("library", "manifest missing"))?;
    let durability = control.durability().ok_or_else(|| {
        Error::unavailable(
            "library durability",
            "directory commit adapter is not configured",
        )
    })?;
    let commits = durability.recover(root)?;
    let mut stats = match control.read_record(root, RecordKind::PlayStats, &song.to_string())? {
        Some(PortableRecord::PlayStats(stats)) => stats,
        _ => PlayStatsRecord {
            song_uuid: song.as_uuid(),
            revision: Revision::INITIAL,
            updated_by_device_id: device,
            hlc: HybridLogicalClock::default(),
            by_device: BTreeMap::new(),
        },
    };
    if let Some(saved) = commits
        .states()
        .find(|state| state.kind == StateKind::PlaybackEvent && state.id == session.as_uuid())
    {
        let event: PlaybackEvent =
            serde_json::from_value(saved.payload.clone()).map_err(|error| {
                Error::unavailable_with_source("playback event", "invalid saved event", error)
            })?;
        if event.song_uuid != song.as_uuid() {
            return Err(Error::conflict("playback session belongs to another song"));
        }
        // A previous projection may have failed; projecting the same absolute
        // bucket snapshot repairs it without creating a second event.
        projection.project_playback(session, song, &stats)?;
        return Ok(false);
    }
    let count = stats.by_device.entry(device.as_uuid()).or_default();
    *count = count.saturating_add(1);
    stats.revision = Revision::from_u64(stats.revision.as_u64().saturating_add(1));
    stats.updated_by_device_id = device;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    stats.hlc = next_hlc(stats.hlc, now.as_secs());
    let envelope = PortableRecordEnvelope::new(PortableRecord::PlayStats(stats.clone()), None)?;
    // The playback session itself is the durable operation identity; the same
    // event must resolve to its original commit even if a response is lost.
    let mut batch = LibraryBatch::new(
        session.as_uuid(),
        manifest.library_id,
        ChangeOrigin::LocalEdit,
        vec![envelope],
    );
    let event = PlaybackEvent {
        song_uuid: song.as_uuid(),
        recorded_at: u64::try_from(now.as_millis()).unwrap_or(u64::MAX),
    };
    let payload = serde_json::to_value(event).map_err(|error| {
        Error::unavailable_with_source("playback event", "cannot encode event", error)
    })?;
    batch.state.push(DurableStateChange::new(
        StateKind::PlaybackEvent,
        session.as_uuid(),
        payload,
    ));
    commit_then_project(control, root, &batch, |_| {
        projection.project_playback(session, song, &stats)
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    use super::*;
    use crate::application::{scan::StartScan, testing::ScanFixture};

    #[derive(Default)]
    struct Projection {
        fail: AtomicBool,
        counts: Mutex<Vec<u64>>,
    }

    impl PlaybackProjection for Projection {
        fn project_playback(
            &self,
            _: PlaybackSessionId,
            _: SongId,
            stats: &PlayStatsRecord,
        ) -> Result<(), Error> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::unavailable("projection", "injected failure"));
            }
            self.counts.lock().unwrap().push(stats.total());
            Ok(())
        }
    }

    #[test]
    fn library_durability_playback_event_stays_private_and_retry_does_not_increment() {
        let fixture = ScanFixture::new();
        fixture.write_file("played.flac", b"audio");
        fixture.set_audio("played.flac", "Played", 1_000);
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .unwrap();
        let song = fixture.all_songs()[0].id();
        let session = PlaybackSessionId::new();
        let projection = Projection::default();
        projection.fail.store(true, Ordering::SeqCst);
        let record = || {
            record_playback(
                &fixture.control,
                &fixture.database,
                &projection,
                fixture.deps.device_id.current_device_id(),
                session,
                song,
            )
        };
        let error = record().unwrap_err();
        assert!(
            matches!(error, Error::LibrarySavedPending { operation, .. } if operation.as_uuid() == session.as_uuid())
        );

        let commits = fixture
            .control
            .durability()
            .unwrap()
            .recover(fixture.root)
            .unwrap();
        let saved = commits
            .commits
            .iter()
            .find(|commit| commit.batch.operation_id == session.as_uuid())
            .unwrap();
        let state = saved
            .batch
            .state
            .iter()
            .find(|state| state.kind == StateKind::PlaybackEvent)
            .unwrap();
        assert_eq!(state.id, session.as_uuid());
        assert_eq!(state.kind.directory(), "playback-events");
        for envelope in &saved.batch.records {
            let shared = serde_json::to_string(envelope).unwrap();
            assert!(!shared.contains(&session.to_string()));
            assert!(!shared.contains("recorded_sessions"));
        }

        projection.fail.store(false, Ordering::SeqCst);
        assert!(!record().unwrap());
        assert_eq!(*projection.counts.lock().unwrap(), vec![1]);
        assert_eq!(
            fixture
                .control
                .durability()
                .unwrap()
                .recover(fixture.root)
                .unwrap()
                .commits
                .len(),
            commits.commits.len()
        );
    }
}
