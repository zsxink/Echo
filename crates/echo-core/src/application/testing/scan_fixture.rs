//! Test composition root for the scan/watch/root-switch use cases.
//!
//! [`ScanFixture`] wires every port double into a [`ScanDeps`] the same way
//! the desktop runtime will wire the real adapters: one shared fake file
//! system, deterministic probe/metadata maps, content-addressed hashing and
//! one shared in-memory database. Tests never touch a real user directory.

use std::borrow::Cow;
use std::sync::Arc;

use crate::application::ports::SongRepository;
use crate::application::scan::{ScanConfig, ScanDeps, ScanSupervisor};
use crate::application::testing::clock::{FakeIdGenerator, ManualClock};
use crate::application::testing::filesystem::FakeLibraryFileSystem;
use crate::application::testing::memory_database::{MemoryDatabase, ScanRunRow};
use crate::application::testing::small_fakes::{
    FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, MemoryControlPlane,
    MemoryCoverCache,
};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, SongId};
use crate::domain::library::MEDIA_ROOT;
use crate::domain::media::{AudioFormat, ParsedMetadata};
use crate::error::Error;

/// Normalize a test's relative path into the managed `media/` tree (portable
/// layout §5: the only scanable content). A path already under `media/` is
/// left untouched; a bare test path (`歌手/晴天.flac`, `a.mp3`) is prefixed.
/// The fake filesystem's `enumerate` mirrors the real walker and only reports
/// `media/` paths, so every fixture helper resolves test paths into that tree —
/// while keeping the test seeding readable.
fn in_media(value: &str) -> Cow<'_, str> {
    if value == MEDIA_ROOT
        || value.starts_with(MEDIA_ROOT) && value.as_bytes().get(MEDIA_ROOT.len()) == Some(&b'/')
    {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(format!("{MEDIA_ROOT}/{value}"))
    }
}

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
    pub control: MemoryControlPlane,
    pub clock: ManualClock,
}

/// Two generations of the same files, as a real library ended up carrying
/// them (design evidence E4): the local rows hold the **newer** identity a
/// later scan re-minted, while `echo/records/songs` still holds the earlier
/// one — disjoint UUIDs, identical `media/` paths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetachedLegacyRecords {
    /// Local rows (newer), as `(library-relative path, local UUID)`.
    pub local: Vec<(String, SongId)>,
    /// Recorded identities (older) for the same paths, in the same order.
    pub recorded: Vec<(String, uuid::Uuid)>,
    /// Favorite records written beside the song records, as `(song UUID, is_favorite)`.
    pub favorites: Vec<(uuid::Uuid, bool)>,
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
        let control_plane_fake = MemoryControlPlane::new();
        let control_plane: Arc<dyn crate::application::ports::ControlPlanePort> =
            Arc::new(control_plane_fake.clone());
        let clock = ManualClock::new();
        let ids: Arc<dyn crate::application::ports::IdGenerator> = Arc::new(FakeIdGenerator::new());
        let clock_dyn: Arc<dyn crate::application::ports::Clock> = Arc::new(clock.clone());
        let deps = Arc::new(ScanDeps {
            roots: Arc::new(database.clone()),
            songs: Arc::new(database.clone()),
            catalog: Arc::new(database.clone()),
            playlists: Arc::new(database.clone()),
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
            control: control_plane,
            device_id: Arc::new(database.clone()),
            sync: Arc::new(database.clone()),
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
            control: control_plane_fake,
            clock,
        }
    }

    /// Write a file into the fixture root's `media/` tree (test setup). A bare
    /// path is resolved under `media/` (portable layout §5).
    pub fn write_file(&self, path: &str, bytes: &[u8]) {
        let base = self
            .fs
            .root_path(self.root)
            .expect("fixture root registered");
        let absolute = base.join(in_media(path).as_ref());
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("create parent directory");
        }
        std::fs::write(absolute, bytes).expect("write fixture file");
    }

    /// Remove a file from the fixture root's `media/` tree (test setup).
    pub fn remove_file(&self, path: &str) {
        let base = self
            .fs
            .root_path(self.root)
            .expect("fixture root registered");
        std::fs::remove_file(base.join(in_media(path).as_ref())).expect("remove fixture file");
    }

    /// A relative path helper, resolved into the managed `media/` tree.
    #[must_use]
    pub fn path(&self, value: &str) -> RelativeMediaPath {
        RelativeMediaPath::new(in_media(value).as_ref()).expect("valid relative path")
    }

    /// Script a successful audio probe + metadata for `path`.
    pub fn set_audio(&self, path: &str, title: &str, duration_ms: u64) {
        self.script_audio(path, title, duration_ms, None);
    }

    /// Like [`set_audio`](Self::set_audio), but the file also carries artwork
    /// embedded in its tags — the case the UI renders through `cover://`
    /// (design §115 内置优先).
    pub fn set_audio_with_cover(&self, path: &str, title: &str, duration_ms: u64, cover: &[u8]) {
        self.script_audio(path, title, duration_ms, Some(cover));
    }

    /// Script the probe + metadata of one file, optionally with embedded artwork.
    /// The maps are keyed by the bare test path (as tests always have); the
    /// fakes' prefix-tolerant readers resolve the scan's `media/…` candidates
    /// back onto those keys, so an explicit `metadata.set` written after
    /// `set_audio` can still override fields like embedded lyrics.
    fn script_audio(&self, path: &str, title: &str, duration_ms: u64, cover: Option<&[u8]>) {
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
                cover: cover.map(|bytes| crate::domain::media::EmbeddedCover {
                    bytes: bytes.to_vec(),
                    mime: "image/png".to_owned(),
                }),
                ..ParsedMetadata::default()
            },
        );
    }

    /// Snapshot of the songs in the fixture root.
    #[must_use]
    pub fn all_songs(&self) -> Vec<Song> {
        SongRepository::all_in_root(&self.database, self.root).expect("song snapshot")
    }

    /// Seed the **detached legacy records** shape that design evidence
    /// E4/E6/E7 measured on a real library: `echo/records/` describes exactly
    /// the same `media/` files as the local database, but under a *disjoint*
    /// set of UUIDs — an earlier generation, re-minted when the app data was
    /// wiped once — and there is no manifest.
    ///
    /// Favorites are written for a subset of the song records, as E6 found.
    /// Returns both identity generations in the same file order.
    #[must_use]
    pub fn seed_detached_legacy_records(&self, files: usize) -> DetachedLegacyRecords {
        use crate::application::portable_materialize::{favorite_record, song_record};
        use crate::application::ports::ControlPlanePort;
        use crate::domain::ids::{Revision, SongId};
        use crate::domain::library::{
            FavoriteRecord, HybridLogicalClock, PortableRecord, SongRecord,
        };

        const HLC: HybridLogicalClock = HybridLogicalClock::new(1_700_000_000, 0);
        let device = self.deps.device_id.current_device_id();
        let mut result = DetachedLegacyRecords::default();
        for index in 0..files {
            let path = format!("歌手/曲目{index:03}.flac");
            let bytes = format!("audio-bytes-{index}").into_bytes();
            self.write_file(&path, &bytes);
            self.set_audio(&path, &format!("曲目{index:03}"), 180_000);
            let media = self.path(&path);

            // The local row: the newer generation a later scan minted.
            let local_id = SongId::new();
            let mut local = Song::new(local_id, self.root, media, Revision::INITIAL);
            local.apply_scan_facts(
                self.deps.hasher.hash_of_bytes(&bytes),
                bytes.len() as u64,
                1,
                AudioFormat::Flac,
            );
            SongRepository::upsert(&self.database, &local).expect("seed a local row");

            // The record: the earlier generation of the same path.
            let recorded_uuid = uuid::Uuid::new_v4();
            let built = song_record(device, HLC, Revision::INITIAL, &local).expect("song record");
            let record = SongRecord {
                song_uuid: recorded_uuid,
                ..built
            };
            self.control
                .write_record(self.root, &PortableRecord::Song(record))
                .expect("write the legacy song record");

            // E6: the favorite records are a strict subset of the song records.
            if index % 3 == 0 {
                let is_favorite = index % 2 == 0;
                self.control
                    .write_record(
                        self.root,
                        &PortableRecord::Favorite(FavoriteRecord {
                            is_favorite,
                            ..favorite_record(
                                device,
                                HLC,
                                Revision::INITIAL,
                                SongId::from_uuid(recorded_uuid),
                                is_favorite,
                            )
                        }),
                    )
                    .expect("write the legacy favorite record");
                result.favorites.push((recorded_uuid, is_favorite));
            }

            result.local.push((path.clone(), local_id));
            result.recorded.push((path, recorded_uuid));
        }
        result
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
