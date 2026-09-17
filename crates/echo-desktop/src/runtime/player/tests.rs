//! Tests for the runtime playback assembly. Shared doubles + helpers live here;
//! per-area cases are split into sibling submodules to keep each file small.

use super::auto_advance::spawn_auto_advance;
use super::coordinated_delete::delete_song_coordinated;
use super::metadata::QueueMetadataResolver;
use super::recorder::spawn_stats_recorder;
use super::session::{restore_or_prime_playback, spawn_session_saver};
use super::snapshot::{map_snapshot, spawn_forwarder};
use super::*;
use crate::player::fake::FakePlayer;
use crate::player::port::PlayerCommand;
use crate::player::queue::{QueueEntry, QueueItem, ViewContext};
use crate::player::recording::PlaybackRecorder;
use crate::player::session::PlaybackSession;
use echo_core::application::ports::{SongRepository, UnitOfWork};
use echo_core::domain::ids::{
    LibraryRootId, PlaybackSessionId, QueueEntryId, RelativeMediaPath, Revision, SongId,
};
use std::sync::Mutex as StdMutex;

/// An in-memory [`SessionPersistence`] double.
struct MemSession(StdMutex<Option<PlaybackSession>>);

impl MemSession {
    fn empty() -> Self {
        Self(StdMutex::new(None))
    }
    fn with(session: PlaybackSession) -> Self {
        Self(StdMutex::new(Some(session)))
    }
}

impl SessionPersistence for MemSession {
    fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String> {
        *self.0.lock().expect("mem session lock") = session.cloned();
        Ok(())
    }
    fn load(&self) -> Result<Option<PlaybackSession>, String> {
        Ok(self.0.lock().expect("mem session lock").clone())
    }
}

/// A recording sink that captures calls (for wiring assertions).
#[derive(Default)]
struct SpySink(StdMutex<Vec<(PlaybackSessionId, SongId)>>);

impl PlaybackRecorder for SpySink {
    fn record(&self, session: PlaybackSessionId, song: SongId) -> Result<bool, String> {
        self.0.lock().unwrap().push((session, song));
        Ok(true)
    }
}

/// A [`SessionPersistence`] double that records every write, so a test can
/// assert both *what* was stored and *how often* the store was touched.
struct CountingSession(StdMutex<(usize, Option<PlaybackSession>)>);

impl CountingSession {
    fn new() -> Self {
        Self(StdMutex::new((0, None)))
    }
    fn saves(&self) -> usize {
        self.0.lock().expect("counting session lock").0
    }
    fn last(&self) -> Option<PlaybackSession> {
        self.0.lock().expect("counting session lock").1.clone()
    }
}

impl SessionPersistence for CountingSession {
    fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String> {
        // Scoped so the count is visible to `load()` before `save` returns.
        {
            let mut guard = self.0.lock().expect("counting session lock");
            guard.0 += 1;
            guard.1 = session.cloned();
        }
        Ok(())
    }
    fn load(&self) -> Result<Option<PlaybackSession>, String> {
        Ok(self.0.lock().expect("counting session lock").1.clone())
    }
}

/// Drive a fake player from `Stopped` to **`Paused` at `volume`** with the
/// saver already attached, and hand back the port plus the recording store.
///
/// Order matters: the saver subscribes to the *live* stream, so it must be
/// attached before anything is published. The position throttle is set to
/// an hour, so once the transport settles the only thing that can make the
/// saver write is the audio change itself — which is what is under test.
fn paused_fake_with_saver(volume: f64) -> (Arc<dyn PlayerPort>, Arc<CountingSession>) {
    let controller = PlayerController::over_fake(FakePlayer::new());
    let store = Arc::new(CountingSession::new());
    spawn_session_saver(
        controller.port.clone(),
        controller.coordinator.clone(),
        store.clone(),
        Arc::new(StdMutex::new(None)),
        std::time::Duration::from_secs(3600),
    );
    controller
        .port
        .send(PlayerCommand::SetVolume(volume))
        .expect("volume");
    controller
        .port
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: PlaybackSessionId::new(),
        })
        .expect("load");
    controller.port.send(PlayerCommand::Pause).expect("pause");
    (controller.port, store)
}

/// Wait (bounded) until `cond` holds, returning whether it did.
fn wait_until(mut cond: impl FnMut() -> bool, budget: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cond()
}

/// A `MemoryDatabase` seeded with one library song (title/artist/duration)
/// and an attached cover, so resolver tests have real metadata to resolve.
fn seeded_db() -> (
    Arc<echo_core::application::testing::memory_database::MemoryDatabase>,
    SongId,
) {
    let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
    let song_id = SongId::new();
    let mut song = echo_core::domain::entities::Song::new(
        song_id,
        LibraryRootId::new(),
        RelativeMediaPath::new("a.flac").expect("path"),
        Revision::INITIAL,
    );
    song.apply_metadata(
        Some("晴天".to_owned()),
        Some("周杰伦".to_owned()),
        Some("叶惠美".to_owned()),
        Some(std::time::Duration::from_secs(239)),
    );
    db.upsert(&song).expect("seed song");
    db.with_tx(Box::new(move |tx| {
        tx.attach_cover(
            song_id,
            &echo_core::application::ports::CoverAssetRef {
                content_hash: "hash".to_owned(),
                mime: "image/png".to_owned(),
                asset_key: "cv1-seeded".to_owned(),
            },
        )
    }))
    .expect("seed cover");
    (db, song_id)
}

mod auto_advance;
mod deletion;
mod forwarder;
mod metadata;
mod restore_prime;
mod session_saver;
mod snapshot;
mod stats;
