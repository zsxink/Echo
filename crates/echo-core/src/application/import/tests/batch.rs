#[test]
fn unreadable_sidecar_gives_audio_success_and_lyrics_failure_without_a_half_sidecar() {
    let g = gated();
    g.sources.add("hit", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("hit", "晴天.lrc", b"[00:01.00]sidecar line");
    g.sources.fail_sidecar("hit", "侧车文件不可读");
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        song,
        target,
        lyrics,
        ..
    } = report.results.first().cloned().expect("one result")
    else {
        panic!("the audio must still import: {:?}", report.results[0]);
    };
    let LyricsImportResult::Failed { code, message } = &*lyrics else {
        panic!("lyrics must be reported as failed: {lyrics:?}");
    };
    assert_eq!(*code, "unavailable");
    assert!(!message.is_empty(), "the reason is user-presentable");

    // 音频成功: the record exists under the reserved UUID.
    let record = g.deps.songs.by_id(song).expect("query").expect("record");
    assert_eq!(record.path(), &target);
    assert!(
        read_library_file(&g, "media/歌手/歌手 - 晴天.flac") == b"audio-bytes",
        "audio published"
    );
    // 不留半侧车: no `.lrc` at the paired target and nothing staged.
    assert!(
        !g.fixture
            .fs
            .root_path(g.fixture.root)
            .unwrap()
            .join("media/歌手/歌手 - 晴天.lrc")
            .exists(),
        "no half or empty sidecar is left behind"
    );
    assert_eq!(g.fixture.fs.staged_count(), 0, "no staged residue");
    // The song must NOT present arbitrary content as its sidecar.
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&g.fixture.database, song)
            .unwrap();
    assert!(
        !candidates
            .iter()
            .any(|c| c.source() == LyricsSource::Sidecar),
        "a failed sidecar import must not wire a sidecar candidate"
    );
    // The failed sidecar never opened a stream (describe failed first).
    assert!(g.sources.sidecar_read_keys().is_empty());
}

#[test]
fn sidecar_publish_conflict_is_audio_success_lyrics_failure_and_keeps_the_incumbent() {
    let g = gated();
    // A foreign `.lrc` already sits at the exact paired target.
    g.fixture
        .write_file("media/歌手/歌手 - 晴天.lrc", b"foreign-lrc");
    g.sources.add("hit", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("hit", "晴天.lrc", b"[00:01.00]my sidecar");
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, lyrics, .. } =
        report.results.first().cloned().expect("one result")
    else {
        panic!("the audio must still import: {:?}", report.results[0]);
    };
    let LyricsImportResult::Failed { code, .. } = &*lyrics else {
        panic!("the sidecar publish must be a lyrics failure: {lyrics:?}");
    };
    assert_eq!(*code, "conflict", "the exclusive publish refuses the pair");

    // 音频成功 / 歌词失败，且不留半侧车：the incumbent keeps its bytes.
    assert_eq!(
        read_library_file(&g, "media/歌手/歌手 - 晴天.lrc"),
        b"foreign-lrc",
        "the occupying file is never replaced"
    );
    assert_eq!(
        read_library_file(&g, "media/歌手/歌手 - 晴天.flac"),
        b"audio-bytes",
        "the audio import still succeeded"
    );
    assert_eq!(g.fixture.fs.staged_count(), 0, "no staged residue");

    // The journal shows two independent per-resource rows: the audio
    // completed and the lyrics rolled back — never one fake success.
    let report_op = report
        .results
        .iter()
        .find_map(|o| match o {
            ImportOutcome::Imported { operation, .. } => Some(*operation),
            _ => None,
        })
        .expect("operation");
    let items = g.deps.journal.items(report_op).expect("journal items");
    assert_eq!(items.len(), 2, "audio + lyrics each carry a row");
    let audio = items
        .iter()
        .find(|i| i.kind == OperationResourceKind::Audio)
        .expect("audio row");
    let lyrics_row = items
        .iter()
        .find(|i| i.kind == OperationResourceKind::Lyrics)
        .expect("lyrics row");
    assert_eq!(audio.state, OperationState::Completed);
    assert_eq!(
        lyrics_row.state,
        OperationState::RolledBack,
        "the sidecar row is individually rolled back"
    );
    assert_eq!(lyrics_row.claim_key, lyrics_row.target_path.identity_key());
    assert_eq!(
        lyrics_row.target_path,
        RelativeMediaPath::new("media/歌手/歌手 - 晴天.lrc").unwrap()
    );
    assert_eq!(
        lyrics_row.source.as_deref(),
        Some("hit#lrc"),
        "the sidecar carries its own logical source locator"
    );
    // The song's sidecar source is cleared (no half attribution).
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&g.fixture.database, song)
            .unwrap();
    assert!(
        !candidates
            .iter()
            .any(|c| c.source() == LyricsSource::Sidecar),
        "the failed sidecar must not leak into the song's candidates"
    );
}

#[test]
fn pending_delete_content_is_not_an_import_conflict() {
    let g = gated();
    let bytes = b"audio-bytes";
    let old_song = seed_song(&g, "media/歌手/歌手 - 晴天.flac", bytes);
    g.deps
        .songs
        .set_availability(old_song.id(), SongAvailability::PendingDelete)
        .expect("mark the old song pending delete");

    // The delete flow has moved the old file into its trash staging area, so
    // the old database path is not occupied by a live library file.
    g.sources.add("hit", "晴天.flac", bytes);
    tagged(&g, bytes, Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, target, .. } = report
        .results
        .first()
        .cloned()
        .expect("one result")
    else {
        panic!("a pending-delete record must not cause duplicate import: {:?}", report.results);
    };

    assert_ne!(song, old_song.id(), "re-import gets a fresh song identity");
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        g.deps
            .songs
            .by_id(old_song.id())
            .expect("old song query")
            .expect("old row remains")
            .availability(),
        SongAvailability::PendingDelete
    );
    assert_eq!(
        g.deps
            .songs
            .by_id(song)
            .expect("new song query")
            .expect("new row")
            .availability(),
        SongAvailability::Available
    );
}

/// A reader double whose sidecar description lies about the content size —
/// a vanished/raced `.lrc` mid-selection (verify the audio still imports).
struct SizeLyingReader {
    inner: FakeImportSources,
}

impl ImportSourceReader for SizeLyingReader {
    fn describe(
        &self,
        source: &ImportSource,
    ) -> Result<crate::application::ports::ImportSourceInfo, Error> {
        self.inner.describe(source)
    }
    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn std::io::Read + 'a>, Error> {
        self.inner.open(source)
    }
    fn sidecar(&self, _source: &ImportSource) -> Result<Option<SidecarInfo>, Error> {
        Ok(Some(SidecarInfo {
            display_name: "晴天.lrc".to_owned(),
            size: 6,
        }))
    }
    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn std::io::Read + 'a>>, Error> {
        self.inner.open_sidecar(source)
    }
}

#[test]
fn sidecar_staging_size_mismatch_is_lyrics_failure_with_audio_committed() {
    let g = gated();
    let lying = SizeLyingReader {
        inner: g.sources.clone(),
    };
    g.sources.add("hit", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("hit", "晴天.lrc", b"[00:01.00]sidecar line"); // 25 bytes
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &lying)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { lyrics, .. } =
        report.results.into_iter().next().expect("one")
    else {
        panic!("the audio must still import");
    };
    let LyricsImportResult::Failed { code, .. } = &*lyrics else {
        panic!("the mismatched sidecar must be a lyrics failure: {lyrics:?}");
    };
    assert_eq!(*code, "corrupt_media");
    assert!(
        !g.fixture
            .fs
            .root_path(g.fixture.root)
            .unwrap()
            .join("media/歌手/歌手 - 晴天.lrc")
            .exists(),
        "a mismatched sidecar never publishes"
    );
    assert_eq!(g.fixture.fs.staged_count(), 0);
}

/// The REAL root-constrained adapter is already exercised by 5.3; task
/// 5.4 adds the same end-to-end source-invariance guarantee for a sidecar:
/// the source `.lrc` is copied, never moved or modified.
#[test]
fn real_fs_import_copies_the_source_lrc_without_moving_or_modifying_it() {
    let external = tempfile::tempdir().expect("external temp dir");
    let library = tempfile::tempdir().expect("library temp dir");
    let source_path = external.path().join("晴天.flac");
    std::fs::write(&source_path, b"original-source-bytes").expect("source");
    let lrc_path = external.path().join("晴天.lrc");
    std::fs::write(&lrc_path, b"[00:01.00]sidecar line").expect("lrc");
    let lrc_modified_before = std::fs::metadata(&lrc_path)
        .expect("stat lrc")
        .modified()
        .expect("mtime");

    let (root, fs) = real_adapter_over(library.path());
    let (probe, metadata) = seeded_reader_fixtures();
    let database = MemoryDatabase::new();
    let deps = real_fs_deps(&fs, probe, metadata, &database);
    let sources = TempFileSources::new();
    sources.add("hit", "晴天.flac", &source_path);
    sources.add_sidecar("hit", "晴天.lrc", &lrc_path);

    let report = PlanImport::new(&deps, &sources)
        .run(root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { target, lyrics, .. } = &report.results[0] else {
        panic!("the import must succeed: {:?}", report.results[0]);
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        &**lyrics,
        &LyricsImportResult::Imported {
            target: RelativeMediaPath::new("media/歌手/歌手 - 晴天.lrc").unwrap()
        }
    );

    // The source `.lrc` keeps its content, name and location unchanged.
    assert_eq!(
        std::fs::read(&lrc_path).expect("source lrc bytes"),
        b"[00:01.00]sidecar line"
    );
    assert_eq!(
        std::fs::metadata(&lrc_path)
            .expect("stat lrc")
            .modified()
            .expect("mtime"),
        lrc_modified_before,
        "the source sidecar was never written"
    );
    // The published pair sits beside each other in the library.
    assert_eq!(
        std::fs::read(library.path().join("media/歌手/歌手 - 晴天.lrc")).expect("published"),
        b"[00:01.00]sidecar line"
    );
    assert_eq!(database.songs().len(), 1);
    let operation = operation_of(&report).expect("operation");
    let items = database.items(operation).expect("journal items");
    assert_eq!(
        items.len(),
        2,
        "audio + lyrics journal rows on the real stack"
    );
    let lyrics_row = items
        .iter()
        .find(|item| item.kind == OperationResourceKind::Lyrics)
        .expect("lyrics row");
    assert_eq!(lyrics_row.state, OperationState::Completed);
    assert_eq!(
        lyrics_row.target_path,
        RelativeMediaPath::new("media/歌手/歌手 - 晴天.lrc").unwrap()
    );
    assert_eq!(
        lyrics_row.expected_hash,
        deps.hasher.hash_of_bytes(b"[00:01.00]sidecar line"),
        "the journal records the sidecar's content hash"
    );
    assert_eq!(
        database.envelope_of(operation).map(|(root, _)| root),
        Some(root)
    );
}

// -----------------------------------------------------------------------
// Task 5.6: dual BLAKE3 dedup and idempotent retry. A plan-time check sees
// the snapshot; a pre-commit re-check (on the full-file hash of the just
// published file) closes the concurrent-import/watcher race so identical
// content never yields a second logical song; and retrying the same content
// is recognized as a duplicate, not re-copied.
// -----------------------------------------------------------------------

/// A fs wrapper that, at the exact moment our import publishes its final
/// audio, injects a concurrent song with the SAME content hash at a
/// *different* path — the race task 5.6 guards against: a concurrent
/// import/watcher committing the same content between the plan-time dedup
/// and our pre-commit re-check must never produce a second logical song.
struct ConcurrentDedupInjector {
    inner: FakeLibraryFileSystem,
    database: MemoryDatabase,
    root: LibraryRootId,
    concurrent: SongId,
    hash: String,
    content: Vec<u8>,
    injected: std::sync::Mutex<bool>,
}

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for ConcurrentDedupInjector {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        self.inner.enumerate(root)
    }
    fn file_meta(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<FileMeta, Error> {
        self.inner.file_meta(root, path)
    }
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        self.inner.read_head(root, path, limit)
    }
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        // The final audio lands first, then the concurrent import surfaces
        // with the SAME content at its own path + record. The once-flag is
        // settled before the blocking file side effects so the guard is not
        // held across them.
        self.inner.publish(root, staged, target)?;
        {
            let mut injected = self.injected.lock().unwrap();
            if *injected {
                return Ok(());
            }
            *injected = true;
        }
        let concurrent_path =
            RelativeMediaPath::new("media/其他/并发 - 同内容.flac").expect("valid path");
        let abs = self
            .inner
            .root_path(self.root)
            .expect("root")
            .join(concurrent_path.normalized());
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("concurrent mkdir");
        }
        std::fs::write(&abs, &self.content).expect("concurrent publish file");
        let mut song = Song::new(
            self.concurrent,
            self.root,
            concurrent_path,
            Revision::INITIAL,
        );
        song.apply_scan_facts(
            self.hash.clone(),
            self.content.len() as u64,
            1,
            AudioFormat::Flac,
        );
        crate::application::ports::SongRepository::upsert(&self.database, &song)
            .expect("inject concurrent song");
        Ok(())
    }
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error> {
        self.inner.stage(root, staged, content)
    }
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error> {
        self.inner.stage_stream(root, staged, content)
    }
    fn read_staged(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
    ) -> Result<Vec<u8>, Error> {
        self.inner.read_staged(root, staged)
    }
    fn discard_staged(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
    ) -> Result<(), Error> {
        self.inner.discard_staged(root, staged)
    }
    fn path_exists(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<bool, Error> {
        self.inner.path_exists(root, path)
    }
    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        self.inner
            .publish_from_staging_path(root, staging_path, target)
    }
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        self.inner.discard_staging_path(root, staging_path)
    }
    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        self.inner.discard_published(root, target)
    }
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        self.inner.trash_path(root, operation, resource_key)
    }
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        self.inner
            .stage_to_trash(root, operation, source, resource_key)
    }
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        self.inner.restore_from_trash(root, trash, target)
    }
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.write_capable(root)
    }
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
        self.inner.establish_write_capability(root)
    }
}

#[test]
fn precommit_dedup_catches_a_concurrent_duplicate_without_a_second_song() {
    let g = gated();
    // The content our import is about to publish.
    let content = b"concurrent-bytes";
    let hash = g.deps.hasher.hash_of_bytes(content);
    let concurrent = crate::domain::ids::SongId::new();
    let injector = Arc::new(ConcurrentDedupInjector {
        inner: g.fixture.fs.clone(),
        database: g.fixture.database.clone(),
        root: g.fixture.root,
        concurrent,
        hash: hash.clone(),
        content: content.to_vec(),
        injected: std::sync::Mutex::new(false),
    });
    let deps = {
        let fs: Arc<dyn LibraryFileSystem> = injector;
        ScanDeps {
            fs,
            ..ScanDeps::clone(&g.deps)
        }
    };
    g.sources.add("hit", "晴天.flac", content);
    tagged(&g, content, Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);

    let report = PlanImport::new(&deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");

    // The import recognized the content was claimed by the concurrent song
    // at pre-commit and contributed nothing: the existing UUID is returned.
    assert_eq!(
        report.results[0],
        ImportOutcome::Duplicate {
            existing: concurrent
        },
        "the concurrent duplicate returns the existing record, never a second song"
    );
    // Exactly ONE logical song for this content.
    let songs = g.fixture.all_songs();
    assert_eq!(
        songs.len(),
        1,
        "identical content has exactly one logical song"
    );
    assert_eq!(
        songs[0].id(),
        concurrent,
        "the surviving record is the concurrent one"
    );
    // Our own published duplicate file was removed (绝不复制/绝无重复文件):
    // only the concurrent song's file remains on disk. And the mixed-batch
    // report for the single input held no committed reservation.
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert!(
        !root_dir.join("media/歌手/歌手 - 晴天.flac").exists(),
        "the duplicate we just published is removed"
    );
    assert!(
        root_dir.join("media/其他/并发 - 同内容.flac").exists(),
        "the concurrent song's file is the one that stays"
    );
    // The concurrent record carries the exact content hash (so the dedup
    // was genuinely driven by BLAKE3, not by an unrelated match).
    assert_eq!(
        songs[0].blake3_hash(),
        Some(hash.as_str()),
        "the surviving record is keyed by the identical BLAKE3"
    );
    // The rolled-back operation released its target claim for a clean retry
    // (one release: the duplicate's own claim, not a committed song's).
    assert_eq!(
        g.fixture.database.released_claims().len(),
        1,
        "the duplicate's rolled-back claim is released"
    );
    assert_eq!(g.fixture.fs.staged_count(), 0, "no staged residue remains");
}

#[test]
fn retry_of_identical_content_is_an_idempotent_duplicate() {
    let g = gated();
    let content = b"retry-bytes";
    g.sources.add("a", "once.flac", content);
    tagged(&g, content, Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    // First run imports the content under a fresh reserved UUID.
    let first = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("a")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        song: first_song, ..
    } = first.results[0]
    else {
        panic!("the first import must succeed");
    };
    assert_eq!(g.fixture.all_songs().len(), 1);

    // A retry of the SAME content (a user retrying, or a second window) is
    // recognized by BLAKE3 on the fresh batch snapshot: no re-copy, no new
    // song — the existing record is returned.
    let second = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("a")])
        .expect("batch-level success");
    assert_eq!(
        second.results[0],
        ImportOutcome::Duplicate {
            existing: first_song
        },
        "the retry returns the completed result as a duplicate"
    );
    // Still exactly one logical song and one file — nothing duplicated.
    assert_eq!(g.fixture.all_songs().len(), 1);
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("published"),
        content,
        "the single file is byte-identical"
    );
    // The first import's claim was released once; the duplicate never held
    // a claim (it was recognized before planning).
    assert_eq!(g.fixture.database.released_claims().len(), 1);
}
