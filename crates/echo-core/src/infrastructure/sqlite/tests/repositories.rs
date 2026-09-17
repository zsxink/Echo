#[test]
fn import_target_claims_are_conditionally_unique_until_release() {
    let stack = ScanStack::new();

    // An in-flight operation holds the active claim on the planned target.
    let blocker = OperationId::new();
    OperationJournalRepository::ensure_operation(
        stack.database.as_ref(),
        blocker,
        stack.root,
        "import",
        None,
    )
    .expect("envelope");
    let claimed = RelativeMediaPath::new("media/歌手/歌手 - 晴天.flac").expect("path");
    stack
        .database
        .upsert_item(
            blocker,
            claim_item(None, claimed.display(), claimed.identity_key()),
        )
        .expect("active claim");

    // The import plans the same target (its batch snapshot predates the
    // claim): the conditional unique index refuses the second claim, so the
    // input fails without publishing anything.
    let content = b"claim-conflict-bytes";
    seed_import_source(&stack, claimed.display(), content, "歌手", "晴天");
    let deps = stack.deps();
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("hit", "晴天.flac", content);
    let source = crate::application::ports::ImportSource::new("hit").expect("key");
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(stack.root, std::slice::from_ref(&source))
        .expect("batch-level success");
    let crate::application::import::ImportOutcome::Failed { code, .. } = &report.results[0] else {
        panic!(
            "the live claim must block a second claim: {:?}",
            report.results[0]
        );
    };
    assert_eq!(
        *code, "conflict",
        "SQLite uniqueness surfaces as a conflict"
    );
    assert!(
        stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query")
            .items
            .is_empty(),
        "no song record while the claim is held by the blocker"
    );
    assert!(
        !stack.library_dir.join(claimed.display()).exists(),
        "nothing was published while the claim was held"
    );

    // After the blocker rolls back (claims released), the same import
    // succeeds under the same planned target with its own reserved identity.
    OperationJournalRepository::release_claims(stack.database.as_ref(), blocker).expect("release");
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(stack.root, &[source])
        .expect("batch-level success");
    let crate::application::import::ImportOutcome::Imported {
        song,
        target,
        operation,
        ..
    } = &report.results[0]
    else {
        panic!(
            "the import succeeds after the claim is released: {:?}",
            report.results[0]
        );
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
    assert!(
        stack.library_dir.join(target.display()).is_file(),
        "the full audio is published"
    );
    assert!(
        crate::application::ports::SongRepository::by_id(stack.database.as_ref(), *song)
            .expect("query")
            .is_some(),
        "the record is committed under the reserved identity"
    );
    // The import's own claim was released at the terminal state.
    let items = stack.database.items(*operation).expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].state,
        crate::domain::state::OperationState::Completed
    );
    assert_eq!(
        items[0].source.as_deref(),
        Some("hit"),
        "the per-item source locator is persisted"
    );
    assert!(
        items[0].staging_path.is_some(),
        "the per-item staging location is persisted"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: scan pipeline against the real SQLite adapter (tasks 4.6/4.7/4.10)
// ---------------------------------------------------------------------------

use crate::application::ports::{
    CoverRepository, LibraryFileSystem as LibraryFileSystemPort, LyricsRepository, OperationItem,
    OperationJournalRepository, RuntimeStateStore, ScanRunRepository, TxWork,
};
use crate::application::root_switch::{
    ActivateLibrary, Blockers, PrepareLibraryCandidate, ROOT_EPOCH_KEY,
};
use crate::application::scan::{ScanConfig, ScanDeps, ScanSupervisor, StartScan};
use crate::application::testing::clock::{FakeIdGenerator, ManualClock, SteppingClock};
use crate::application::testing::filesystem::FakeLibraryFileSystem;
use crate::application::testing::small_fakes::{
    FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, FakeTrash,
    MemoryControlPlane,
};
use crate::application::trash::{finalize_persisted_trash, FinalizeExpiredDeletes};
use crate::domain::state::scan::ScanState as ScanRunState;
use crate::infrastructure::metadata::DiskCoverCache;

#[derive(Clone)]
struct RollbackAfterWrites {
    database: Arc<SqliteDatabase>,
}

impl UnitOfWork for RollbackAfterWrites {
    fn with_tx(&self, work: TxWork) -> Result<(), crate::error::Error> {
        self.database.with_tx(Box::new(move |tx| {
            work(tx)?;
            Err(crate::error::Error::unavailable(
                "database",
                "simulated transaction failure",
            ))
        }))
    }
}

/// A real SQLite database wired to fake file-system/metadata adapters — the
/// composition the desktop runtime will build, minus the OS pieces. One
/// `Arc<SqliteDatabase>` serves every repository port view, exactly like the
/// production composition root.
struct ScanStack {
    _directory: tempfile::TempDir,
    library_dir: std::path::PathBuf,
    database: Arc<SqliteDatabase>,
    fs: Arc<FakeLibraryFileSystem>,
    fs_port: Arc<dyn LibraryFileSystemPort>,
    probe: FakeMediaProbe,
    metadata: FakeMetadataReader,
    root: LibraryRootId,
}

impl ScanStack {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = Arc::new(
            SqliteDatabase::open(directory.path().join("echo.db")).expect("open database"),
        );
        let root = LibraryRootId::new();
        let library_dir = directory.path().join("library");
        std::fs::create_dir_all(&library_dir).expect("library dir");
        LibraryRepository::upsert(
            database.as_ref(),
            &LibraryRoot::new(root, library_dir.clone(), true, true),
        )
        .expect("insert active root");
        let fs = FakeLibraryFileSystem::with_root(root);
        fs.add_root_at(root, library_dir.clone());
        let fs_port: Arc<dyn LibraryFileSystemPort> = Arc::new(fs.clone());
        Self {
            _directory: directory,
            library_dir,
            database,
            fs: Arc::new(fs),
            fs_port,
            probe: FakeMediaProbe::new(),
            metadata: FakeMetadataReader::new(),
            root,
        }
    }

    fn deps(&self) -> ScanDeps {
        // One `Arc<SqliteDatabase>` coerced into every port view.
        let db: Arc<SqliteDatabase> = Arc::clone(&self.database);
        let db_root: Arc<dyn LibraryRepository> = db.clone();
        let db_songs: Arc<dyn SongRepository> = db.clone();
        let db_catalog: Arc<dyn CatalogQueryRepository> = db.clone();
        let db_playlists: Arc<dyn PlaylistRepository> = db.clone();
        let db_lyrics: Arc<dyn LyricsRepository> = db.clone();
        let db_covers: Arc<dyn CoverRepository> = db.clone();
        let db_runs: Arc<dyn ScanRunRepository> = db.clone();
        let db_journal: Arc<dyn OperationJournalRepository> = db.clone();
        let db_uow: Arc<dyn UnitOfWork> = db.clone();
        let db_device: Arc<dyn DeviceIdProvider> = db.clone();
        let db_sync: Arc<dyn SyncStateReader> = db;
        ScanDeps {
            roots: db_root,
            songs: db_songs,
            catalog: db_catalog,
            playlists: db_playlists,
            lyrics: db_lyrics,
            covers: db_covers,
            runs: db_runs,
            journal: db_journal,
            uow: db_uow,
            fs: Arc::clone(&self.fs_port),
            probe: Arc::new(self.probe.clone()),
            metadata: Arc::new(self.metadata.clone()),
            hasher: Arc::new(FakeFileHasher::new(Arc::clone(&self.fs_port))),
            lyrics_parser: Arc::new(FakeLyricsParser::new()),
            cover_cache: Arc::new(
                DiskCoverCache::new(tempfile::TempDir::new().expect("cache dir").keep())
                    .expect("cover cache"),
            ),
            ids: Arc::new(FakeIdGenerator::new()),
            control: Arc::new(MemoryControlPlane::new()),
            device_id: db_device,
            sync: db_sync,
            clock: Arc::new(ManualClock::new()),
            config: ScanConfig {
                batch_size: 2,
                worker_threads: 1,
                ..ScanConfig::default()
            },
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        // The fake filesystem's `enumerate` only discovers `media/` (mirroring
        // the real walker, portable layout §5), so seeds land under `media/`.
        let absolute = if path.starts_with("media/") {
            self.library_dir.join(path)
        } else {
            self.library_dir.join(format!("media/{path}"))
        };
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(absolute, bytes).expect("write file");
    }
}

fn seed_trash_operation(
    stack: &ScanStack,
    state: OperationState,
) -> (ScanDeps, Song, PlaylistId, OperationId) {
    let deps = stack.deps();
    let song = song(stack.root, "media/song.flac", "Song", "Artist");
    SongRepository::upsert(stack.database.as_ref(), &song).expect("seed song");
    SongRepository::set_availability(
        stack.database.as_ref(),
        song.id(),
        SongAvailability::PendingDelete,
    )
    .expect("hide pending song");
    let playlist = PlaylistId::new();
    stack
        .database
        .create(playlist, stack.root, "delete test")
        .expect("playlist");
    stack
        .database
        .add_member(playlist, song.id(), 0)
        .expect("member");

    let operation = OperationId::new();
    let staging =
        RelativeMediaPath::new(&format!("media/trash/{operation}/audio")).expect("stage path");
    stack.write(staging.display(), b"staged-song");
    let expected_hash = deps.hasher.hash(stack.root, &staging).expect("stage hash");
    OperationJournalRepository::ensure_operation(
        stack.database.as_ref(),
        operation,
        stack.root,
        crate::application::delete::DELETE_OPERATION,
        Some(song.id()),
    )
    .expect("operation envelope");
    OperationJournalRepository::upsert_item(
        stack.database.as_ref(),
        operation,
        OperationItem {
            kind: OperationResourceKind::Audio,
            state,
            song: Some(song.id()),
            source: None,
            staging_path: Some(staging),
            target_path: song.path().clone(),
            expected_hash,
            item_key: "audio".to_owned(),
            claim_key: "audio".to_owned(),
        },
    )
    .expect("journal item");
    (deps, song, playlist, operation)
}

fn claim_active(database: &SqliteDatabase, operation: OperationId) -> i64 {
    database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT claim_active FROM operation_items WHERE operation_uuid = ?1 AND item_key = 'audio'",
                    params![operation.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .expect("claim query")
}

#[test]
fn sqlite_trash_forward_finalizes_song_relationships_journal_and_claim() {
    let stack = ScanStack::new();
    let (deps, song, playlist, operation) =
        seed_trash_operation(&stack, OperationState::TrashPending);
    let trash = FakeTrash::new();

    let report = FinalizeExpiredDeletes::new(&deps, &trash)
        .run(stack.root)
        .expect("forward delete");

    assert_eq!(report.finalized, vec![operation]);
    assert_eq!(trash.calls(), vec![operation]);
    assert!(SongRepository::by_id(stack.database.as_ref(), song.id())
        .expect("song query")
        .is_none());
    assert!(stack
        .database
        .members(playlist)
        .expect("members")
        .is_empty());
    assert!(
        OperationJournalRepository::items(stack.database.as_ref(), operation)
            .expect("journal items")
            .iter()
            .all(|item| item.state == OperationState::DatabaseFinalized)
    );
    assert_eq!(claim_active(stack.database.as_ref(), operation), 0);
}

#[test]
fn sqlite_trash_finalization_rollback_keeps_song_journal_and_claim_together() {
    let stack = ScanStack::new();
    let (mut deps, song, playlist, operation) =
        seed_trash_operation(&stack, OperationState::TrashApplied);
    deps.uow = Arc::new(RollbackAfterWrites {
        database: Arc::clone(&stack.database),
    });

    let items = OperationJournalRepository::items(stack.database.as_ref(), operation)
        .expect("journal items");
    assert!(finalize_persisted_trash(&deps, operation, &items).is_err());

    let stored = SongRepository::by_id(stack.database.as_ref(), song.id())
        .expect("song query")
        .expect("rollback keeps song");
    assert_eq!(stored.availability(), SongAvailability::PendingDelete);
    assert_eq!(stack.database.members(playlist).expect("members").len(), 1);
    assert!(
        OperationJournalRepository::items(stack.database.as_ref(), operation)
            .expect("journal items")
            .iter()
            .all(|item| item.state == OperationState::TrashApplied)
    );
    assert_eq!(claim_active(stack.database.as_ref(), operation), 1);
}

#[test]
fn scan_pipeline_persists_songs_lyrics_covers_and_progress() {
    let stack = ScanStack::new();
    stack.write("a.mp3", b"audio-a");
    stack.write("华语/b.flac", b"audio-b");
    stack.write("华语/b.lrc", b"[00:01.00]sidecar");
    for (path, title) in [("a.mp3", "A"), ("华语/b.flac", "B")] {
        stack.probe.set(
            path,
            crate::application::ports::ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
                duration: Some(Duration::from_secs(1)),
            },
        );
        stack.metadata.set(
            path,
            crate::domain::media::ParsedMetadata {
                title: Some(title.to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(Duration::from_secs(1)),
                format: crate::domain::media::AudioFormat::Flac,
                embedded_lyrics: Some("[00:02.00]embedded".to_owned()),
                cover: Some(crate::domain::media::EmbeddedCover {
                    bytes: b"cover-".repeat(64),
                    mime: "image/png".to_owned(),
                }),
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
    }
    let stack_deps = stack.deps();
    let summary = StartScan::new(&stack_deps, &ScanSupervisor::new())
        .run(stack.root)
        .expect("scan ok");
    assert_eq!(summary.progress.created, 2);
    assert_eq!(summary.progress.state, ScanRunState::Completed);

    // Songs persisted with scan facts and visible to the catalog query.
    let page = stack
        .database
        .query_active_songs("", SongSort::default(), None, 100)
        .expect("query");
    assert_eq!(page.items.len(), 2);
    let song = page
        .items
        .iter()
        .find(|song| song.path().display() == "media/a.mp3")
        .expect("song");
    assert_eq!(song.title(), Some("A"));
    assert!(song.blake3_hash().is_some(), "scan facts persisted");
    assert_eq!(song.file_size(), Some(7));

    // Lyrics candidates per source, persisted transactionally with the song.
    let candidates = stack.database.candidates(song.id()).expect("candidates");
    assert!(candidates
        .iter()
        .any(|candidate| candidate.source() == crate::domain::entities::LyricsSource::Embedded));
    // The sidecar pairs with the *other* file (华语/b.flac + 华语/b.lrc).
    let song_b = page
        .items
        .iter()
        .find(|song| song.path().display() == "media/华语/b.flac")
        .expect("song b");
    let candidates_b = stack.database.candidates(song_b.id()).expect("candidates");
    let sources_b: Vec<_> = candidates_b
        .iter()
        .map(crate::domain::entities::LyricsCandidate::source)
        .collect();
    assert!(sources_b.contains(&crate::domain::entities::LyricsSource::Embedded));
    assert!(sources_b.contains(&crate::domain::entities::LyricsSource::Sidecar));

    // Cover reference + asset key consistency.
    let cover = stack
        .database
        .cover_of(song.id())
        .expect("cover")
        .expect("attached");
    assert!(cover.asset_key.starts_with("cv1-"));
    assert!(!stack
        .database
        .referenced_asset_keys(stack.root)
        .expect("keys")
        .is_empty());

    // Scan runs + issues persisted.
    let (state, progress, finished) = stack
        .database
        .scan_run_snapshot(stack.root, 1)
        .expect("run row")
        .expect("row");
    assert_eq!(state, ScanRunState::Completed);
    assert!(finished);
    assert_eq!(progress.discovered, 2);
    assert_eq!(
        stack.database.latest_generation(stack.root).expect("gen"),
        Some(1)
    );

    // Search works on the freshly scanned library (Unicode-safe path too).
    let page = stack
        .database
        .search_active_songs("歌手", SongSort::default(), None, 100)
        .expect("search");
    assert_eq!(page.items.len(), 2);
}

/// Probe double that stalls each probe so a scan stays in flight.
struct SlowProbe {
    inner: FakeMediaProbe,
    delay: Duration,
}

impl crate::application::ports::MediaProbe for SlowProbe {
    fn probe(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<crate::application::ports::ProbeOutcome, Error> {
        std::thread::sleep(self.delay);
        self.inner.probe(root, path)
    }
}

#[test]
fn scan_pipeline_keeps_ui_queries_available_during_scan() {
    let stack = ScanStack::new();
    for index in 0..20 {
        let path = format!("file{index}.mp3");
        stack.write(&path, format!("audio-{index}").as_bytes());
        stack.probe.set(
            &path,
            crate::application::ports::ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
                duration: Some(Duration::from_secs(1)),
            },
        );
        stack.metadata.set(
            &path,
            crate::domain::media::ParsedMetadata {
                title: Some(format!("T{index}")),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(Duration::from_secs(1)),
                format: crate::domain::media::AudioFormat::Flac,
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
    }
    // Slow the workers so the scan runs long enough to query against.
    let mut deps = stack.deps();
    deps.config.batch_size = 4;
    deps.config.worker_threads = 2;
    deps.probe = Arc::new(SlowProbe {
        inner: stack.probe.clone(),
        delay: Duration::from_millis(10),
    });

    let scan_deps = Arc::new(deps);
    let worker_deps = Arc::clone(&scan_deps);
    let supervisor = ScanSupervisor::new();
    let worker_supervisor = supervisor;
    let root = stack.root;
    let scanner =
        std::thread::spawn(move || StartScan::new(&worker_deps, &worker_supervisor).run(root));

    // While the scan is in flight, catalog queries keep succeeding (readers
    // are never blocked by the writer's small batches).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut queries = 0;
    while !scanner.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "scan did not finish in time"
        );
        let page = stack
            .database
            .query_active_songs("", SongSort::default(), None, 100)
            .expect("query during scan");
        assert!(page.items.len() <= 20);
        queries += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    let summary = scanner.join().unwrap().expect("scan ok");
    assert_eq!(summary.progress.created, 20);
    assert!(queries > 0, "queries ran while the scan was in flight");
    let final_page = stack
        .database
        .query_active_songs("", SongSort::default(), None, 100)
        .expect("query after scan");
    assert_eq!(final_page.items.len(), 20);
}
