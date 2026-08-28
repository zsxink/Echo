//! Test composition root for the scan/watch/root-switch use cases.
//!
//! [`ScanFixture`] wires every port double into a [`ScanDeps`] the same way
//! the desktop runtime will wire the real adapters: one shared fake file
//! system, deterministic probe/metadata maps, content-addressed hashing and
//! one shared in-memory database. Tests never touch a real user directory.

use std::sync::Arc;

use crate::application::ports::SongRepository;
use crate::application::scan::{ScanConfig, ScanDeps, ScanSupervisor};
use crate::application::testing::clock::{FakeIdGenerator, ManualClock};
use crate::application::testing::filesystem::FakeLibraryFileSystem;
use crate::application::testing::memory_database::{MemoryDatabase, ScanRunRow};
use crate::application::testing::small_fakes::{
    FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, MemoryCoverCache,
};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::media::{AudioFormat, ParsedMetadata};
use crate::error::Error;

/// A fully wired in-memory library runtime for use-case tests. Every
/// repository view shares one `MemoryDatabase` store, exactly like the real
/// SQLite stack shares one file.
#[derive(Clone)]
pub struct ScanFixture {
    pub root: LibraryRootId,
    pub deps: Arc<ScanDeps>,
    pub supervisor: ScanSupervisor,
    pub fs: FakeLibraryFileSystem,
    pub probe: FakeMediaProbe,
    pub metadata: FakeMetadataReader,
    pub database: MemoryDatabase,
    pub clock: ManualClock,
}

impl Default for ScanFixture {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanFixture {
    /// Build the fixture with one registered root.
    #[must_use]
    pub fn new() -> Self {
        let root = LibraryRootId::new();
        let fake_fs = FakeLibraryFileSystem::with_root(root);
        let fs: Arc<dyn crate::application::ports::LibraryFileSystem> = Arc::new(fake_fs.clone());
        let probe = FakeMediaProbe::new();
        let metadata = FakeMetadataReader::new();
        let hasher: Arc<dyn crate::application::ports::ContentHasher> =
            Arc::new(FakeFileHasher::new(Arc::clone(&fs)));
        let lyrics_parser: Arc<dyn crate::application::ports::LyricsParser> =
            Arc::new(FakeLyricsParser::new());
        let database = MemoryDatabase::new();
        let cover_cache: Arc<dyn crate::application::ports::CoverCache> =
            Arc::new(MemoryCoverCache::new());
        let clock = ManualClock::new();
        let ids: Arc<dyn crate::application::ports::IdGenerator> = Arc::new(FakeIdGenerator::new());
        let clock_dyn: Arc<dyn crate::application::ports::Clock> = Arc::new(clock.clone());
        let deps = Arc::new(ScanDeps {
            roots: Arc::new(database.clone()),
            songs: Arc::new(database.clone()),
            lyrics: Arc::new(database.clone()),
            covers: Arc::new(database.clone()),
            runs: Arc::new(database.clone()),
            journal: Arc::new(database.clone()),
            uow: Arc::new(database.clone()),
            fs,
            probe: Arc::new(probe.clone()),
            metadata: Arc::new(metadata.clone()),
            hasher,
            lyrics_parser,
            cover_cache,
            ids,
            clock: clock_dyn,
            config: ScanConfig {
                batch_size: 2,
                worker_threads: 1,
                ..ScanConfig::default()
            },
        });
        Self {
            root,
            deps,
            supervisor: ScanSupervisor::new(),
            fs: fake_fs,
            probe,
            metadata,
            database,
            clock,
        }
    }

    /// Write a file into the fixture root (test setup).
    pub fn write_file(&self, path: &str, bytes: &[u8]) {
        let base = self
            .fs
            .root_path(self.root)
            .expect("fixture root registered");
        let absolute = base.join(path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("create parent directory");
        }
        std::fs::write(absolute, bytes).expect("write fixture file");
    }

    /// Remove a file from the fixture root (test setup).
    pub fn remove_file(&self, path: &str) {
        let base = self
            .fs
            .root_path(self.root)
            .expect("fixture root registered");
        std::fs::remove_file(base.join(path)).expect("remove fixture file");
    }

    /// A relative path helper.
    #[must_use]
    pub fn path(&self, value: &str) -> RelativeMediaPath {
        RelativeMediaPath::new(value).expect("valid relative path")
    }

    /// Script a successful audio probe + metadata for `path`.
    pub fn set_audio(&self, path: &str, title: &str, duration_ms: u64) {
        self.probe.set(
            path,
            crate::application::ports::ProbeOutcome::Audio {
                format: AudioFormat::Flac,
                duration: Some(std::time::Duration::from_millis(duration_ms)),
            },
        );
        self.metadata.set(
            path,
            ParsedMetadata {
                title: Some(title.to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(std::time::Duration::from_millis(duration_ms)),
                format: AudioFormat::Flac,
                ..ParsedMetadata::default()
            },
        );
    }

    /// Snapshot of the songs in the fixture root.
    #[must_use]
    pub fn all_songs(&self) -> Vec<Song> {
        SongRepository::all_in_root(&self.database, self.root).expect("song snapshot")
    }

    /// One run row of the fixture root.
    #[must_use]
    pub fn run_row(&self, generation: u64) -> Option<ScanRunRow> {
        self.database.run(self.root, generation)
    }

    /// Per-file issues recorded for one run.
    #[must_use]
    pub fn issues(&self, generation: u64) -> Vec<crate::domain::entities::MediaDiagnostic> {
        self.database.issues_of(self.root, generation)
    }
}

/// Convenience: look one song up by path.
pub fn song_by_path(
    fixture: &ScanFixture,
    path: &RelativeMediaPath,
) -> Result<Option<Song>, Error> {
    SongRepository::by_path(&fixture.database, fixture.root, path)
}
