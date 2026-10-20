use super::*;

/// Hasher double that cancels the scan through the supervisor on the
/// first hash call — the same path `CancelScan` takes.
struct CancelOnHash {
    supervisor: ScanSupervisor,
    inner: Arc<dyn ContentHasher>,
}

impl ContentHasher for CancelOnHash {
    fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error> {
        let _cancelled = self.supervisor.cancel(root);
        self.inner.hash(root, path)
    }
    fn hash_of_bytes(&self, bytes: &[u8]) -> String {
        self.inner.hash_of_bytes(bytes)
    }
}

/// Probe double tracking concurrent entries (bounded-worker assertion).
struct CountingProbe {
    inner: Arc<dyn MediaProbe>,
    current: Arc<std::sync::atomic::AtomicUsize>,
    max_seen: Arc<std::sync::atomic::AtomicUsize>,
}

impl MediaProbe for CountingProbe {
    fn probe(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error> {
        let now = self
            .current
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.max_seen
            .fetch_max(now, std::sync::atomic::Ordering::SeqCst);
        // Hold the slot long enough for a second worker to overlap.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let outcome = self.inner.probe(root, path);
        self.current
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        outcome
    }

    fn probe_bytes(&self, content: &[u8], extension: Option<&str>) -> Result<ProbeOutcome, Error> {
        self.inner.probe_bytes(content, extension)
    }
}

use crate::application::delete::DeleteSongs;
use crate::application::ports::{PlaylistRepository, SongRepository};
use crate::application::testing::{scan_fixture::song_by_path, ScanFixture};
use crate::domain::entities::{Song, SongAvailability};
use crate::domain::ids::{PlaylistId, Revision, SongId};

/// Seed one playable song with favorite, one play and a playlist
/// membership, returning its [`SongId`] and the playlist id — the
/// relationship set that task 5.9 proves survives external deletion and
/// recovery.
fn seed_loved_song(fixture: &ScanFixture, path: &str) -> (SongId, PlaylistId) {
    fixture.write_file(path, b"audio-bytes");
    fixture.set_audio(path, "晴天", 269_000);
    let mut song = Song::new(
        SongId::new(),
        fixture.root,
        fixture.path(path),
        Revision::INITIAL,
    );
    song.apply_scan_facts(
        fixture.deps.hasher.hash_of_bytes(b"audio-bytes"),
        11,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    song.set_favorite(true);
    song.record_play();
    SongRepository::upsert(&fixture.database, &song).expect("seed song");

    let playlist = PlaylistId::new();
    fixture
        .database
        .create(playlist, fixture.root, "favorites")
        .expect("create playlist");
    fixture
        .database
        .add_member(playlist, song.id(), 0)
        .expect("add member");
    (song.id(), playlist)
}

fn start_scan(fixture: &ScanFixture) -> StartScan<'_> {
    StartScan::new(&fixture.deps, &fixture.supervisor)
}

#[path = "tests_convergence.rs"]
mod convergence;
#[path = "tests_pipeline.rs"]
mod pipeline;
#[path = "tests_relink.rs"]
mod relink;
