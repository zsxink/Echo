// -----------------------------------------------------------------------
// Task 5.3: streaming copy + BLAKE3, per-resource journal fields, the
// exclusive publish boundary, staged-copy cleanup, DB visibility only
// after the full audio publish, source invariance and staging isolation —
// verified with injected temp directories, never the real user home.
// -----------------------------------------------------------------------

#[test]
fn import_streams_the_copy_and_computes_blake3_during_it() {
    let g = gated();
    // Larger than the 64 KiB copy chunk: the chunked pump is exercised
    // end to end and the digest must equal the full-content BLAKE3.
    let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    g.sources.add("big", "大文件.flac", &content);
    tagged(&g, &content, Some("歌手"), Some("大文件"));
    g.fixture
        .set_audio("media/歌手/歌手 - 大文件.flac", "大文件", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("big")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, target, .. } = &report.results[0] else {
        panic!("the streamed input must import");
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 大文件.flac");

    // The published file holds every byte of the streamed copy.
    let published = g
        .fixture
        .fs
        .root_path(g.fixture.root)
        .expect("root")
        .join("media/歌手/歌手 - 大文件.flac");
    assert_eq!(
        std::fs::read(&published).expect("published bytes"),
        content,
        "the streaming copy is byte-exact"
    );
    // The journal carries the BLAKE3 computed during the copy.
    let operation = operation_of(&report).expect("operation");
    let items = g.deps.journal.items(operation).expect("journal items");
    let expected_hash = g.deps.hasher.hash_of_bytes(&content);
    assert_eq!(
        items.first().map(|item| item.expected_hash.as_str()),
        Some(expected_hash.as_str()),
    );
    // The committed record carries the same content hash and identity.
    let record = g.deps.songs.by_id(*song).expect("query").expect("record");
    assert_eq!(record.blake3_hash(), Some(expected_hash.as_str()));
    // The source was streamed exactly once (no second planning read).
    assert_eq!(g.sources.read_keys(), vec!["big".to_owned()]);
}

#[test]
fn journal_item_records_source_staging_target_and_expected_hash() {
    let g = gated();
    g.sources.add("good", "晴天.flac", b"journal-bytes");
    tagged(&g, b"journal-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("good")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        operation,
        song,
        target,
        ..
    } = report.results.into_iter().next().expect("one result")
    else {
        panic!("the input must import");
    };

    // The per-resource item (design §8: 逐资源源定位/暂存/目标/hash).
    let items = g.deps.journal.items(operation).expect("journal items");
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.kind, OperationResourceKind::Audio);
    assert_eq!(item.state, OperationState::Completed);
    assert_eq!(item.song, Some(song), "UUID equals the reservation");
    assert_eq!(
        item.source.as_deref(),
        Some("good"),
        "the logical source locator is recorded, never a path"
    );
    let staging = item.staging_path.as_ref().expect("staged location");
    assert!(
        staging
            .display()
            .starts_with(crate::domain::library::STAGING_ROOT),
        "staged inside the operation's slot: {}",
        staging.display()
    );
    assert!(
        staging.display().ends_with("/audio"),
        "one slot per operation resource: {}",
        staging.display()
    );
    assert_eq!(item.target_path, target);
    assert_eq!(item.claim_key, target.identity_key());
    assert_eq!(
        item.expected_hash,
        g.deps.hasher.hash_of_bytes(b"journal-bytes"),
    );
    // The operation envelope exists and is bound to the root (design §8:
    // 每个 operation 由总状态和逐资源 operation_items 组成).
    assert_eq!(
        g.fixture
            .database
            .envelope_of(operation)
            .map(|(root, _)| root),
        Some(g.fixture.root)
    );
    // Terminal state released the claim.
    assert_eq!(g.fixture.database.released_claims(), vec![operation]);
}

#[test]
fn database_record_is_visible_only_after_the_full_audio_is_published() {
    let fixture = ScanFixture::new();
    let probe = Arc::new(VisibilityProbe {
        inner: fixture.fs.clone(),
        songs: fixture.database.clone(),
        root: fixture.root,
        events: std::sync::Mutex::new(Vec::new()),
    });
    let deps = {
        let fs: Arc<dyn LibraryFileSystem> = probe.clone();
        ScanDeps {
            fs,
            ..ScanDeps::clone(&fixture.deps)
        }
    };
    let sources = FakeImportSources::new();
    sources.add("good", "晴天.flac", b"visible-bytes");
    tag_content(&fixture, b"visible-bytes", Some("歌手"), Some("晴天"));
    fixture.set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&deps, &sources)
        .run(fixture.root, &[source("good")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, .. } = report.results.into_iter().next().expect("one")
    else {
        panic!("the input must import");
    };

    // The song table was empty at staging time AND at publish time: the
    // record only becomes visible through the commit afterwards.
    assert_eq!(
        probe.events(),
        vec![("stage_stream", 0), ("publish", 0)],
        "no database record before the full audio publish"
    );
    // The published audio is complete (byte-identical), then committed
    // under the reserved identity.
    let published = fixture
        .fs
        .root_path(fixture.root)
        .expect("root")
        .join("media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        std::fs::read(&published).expect("published"),
        b"visible-bytes"
    );
    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), song);
    assert_eq!(songs[0].path().display(), "media/歌手/歌手 - 晴天.flac");
}

#[test]
fn duplicate_and_failed_inputs_discard_their_staged_copies() {
    let g = gated();
    let existing = seed_song(&g, "media/已有/other.flac", b"duplicate-content");
    // The duplicate's staged copy is created (dedup runs after the
    // streaming copy) and must be discarded without a claim.
    g.sources.add("dup", "重复.flac", b"duplicate-content");
    // The failed input hits a publish-time conflict (a file appears at
    // its planned target between the batch snapshot and the publish).
    let planter = Arc::new(RacePlanter {
        inner: g.fixture.fs.clone(),
        plant: std::sync::Mutex::new(Some("media/歌手/歌手 - 晴天.flac".to_owned())),
    });
    let deps = {
        let fs: Arc<dyn LibraryFileSystem> = planter;
        ScanDeps {
            fs,
            ..ScanDeps::clone(&g.deps)
        }
    };
    g.sources.add("new", "晴天.flac", b"fresh-bytes");
    tagged(&g, b"fresh-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&deps, &g.sources)
        .run(g.fixture.root, &[source("dup"), source("new")])
        .expect("batch-level success");
    assert_eq!(
        report.results[0],
        ImportOutcome::Duplicate {
            existing: existing.id()
        }
    );
    let ImportOutcome::Failed { code, .. } = &report.results[1] else {
        panic!("the raced input must fail: {:?}", report.results[1]);
    };
    assert_eq!(*code, "conflict");

    // No staged copy leaks and only the failed input ever held a claim.
    assert_eq!(
        g.fixture.fs.staged_count(),
        0,
        "staged copies are discarded"
    );
    assert_eq!(g.fixture.database.released_claims().len(), 1);
    assert_eq!(g.fixture.all_songs().len(), 1, "only the seed remains");
    // The conflicting file keeps its bytes (never overwritten).
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("planted"),
        b"planted-first"
    );
}

/// The REAL root-constrained adapter over an injected temp library, with
/// its controlled staging directory established (activation grants write
/// capability by creating it — design §8: 首次获得写能力时
/// exclusive-create; the runtime wires this, the test mirrors it).
fn real_adapter_over(library: &std::path::Path) -> (LibraryRootId, Arc<dyn LibraryFileSystem>) {
    let root = LibraryRootId::new();
    let registry = crate::infrastructure::filesystem::registry::RootRegistry::new();
    registry.register(root, library);
    let real_fs =
        crate::infrastructure::filesystem::adapter::RootConstrainedFileSystem::new(registry);
    real_fs
        .staging()
        .ensure_dir(root)
        .expect("staging established");
    (root, Arc::new(real_fs))
}

/// Deterministic probe/tags fixtures for the full-stack import test: the
/// published path probes as audio, and the source content parses to the
/// 歌手/晴天 tags that drive the target name.
fn seeded_reader_fixtures() -> (FakeMediaProbe, FakeMetadataReader) {
    let tags = ParsedMetadata {
        artist: Some("歌手".to_owned()),
        title: Some("晴天".to_owned()),
        ..ParsedMetadata::default()
    };
    let probe = FakeMediaProbe::new();
    probe.set(
        "media/歌手/歌手 - 晴天.flac",
        crate::application::ports::ProbeOutcome::Audio {
            format: AudioFormat::Flac,
            duration: Some(std::time::Duration::from_secs(1)),
        },
    );
    let metadata = FakeMetadataReader::new();
    metadata.set("media/歌手/歌手 - 晴天.flac", tags.clone());
    metadata.set_bytes(b"original-source-bytes", tags);
    (probe, metadata)
}

/// `ScanDeps` over the REAL root-constrained adapter (an injected temp
/// library) with the deterministic doubles everywhere else.
fn real_fs_deps(
    fs: &Arc<dyn LibraryFileSystem>,
    probe: FakeMediaProbe,
    metadata: FakeMetadataReader,
    database: &MemoryDatabase,
) -> ScanDeps {
    ScanDeps {
        roots: Arc::new(database.clone()),
        songs: Arc::new(database.clone()),
        catalog: Arc::new(database.clone()),
        playlists: Arc::new(database.clone()),
        lyrics: Arc::new(database.clone()),
        covers: Arc::new(database.clone()),
        runs: Arc::new(database.clone()),
        journal: Arc::new(database.clone()),
        uow: Arc::new(database.clone()),
        fs: Arc::clone(fs),
        probe: Arc::new(probe),
        metadata: Arc::new(metadata),
        hasher: Arc::new(FakeFileHasher::new(Arc::clone(fs))),
        lyrics_parser: Arc::new(FakeLyricsParser::new()),
        cover_cache: Arc::new(MemoryCoverCache::new()),
        control: Arc::new(MemoryControlPlane::new()),
        device_id: Arc::new(database.clone()),
        sync: Arc::new(database.clone()),
        ids: Arc::new(FakeIdGenerator::new()),
        clock: Arc::new(ManualClock::new()),
        config: ScanConfig::default(),
    }
}

#[test]
fn source_files_stay_unchanged_and_foreign_staging_names_are_isolated() {
    let external = tempfile::tempdir().expect("external temp dir");
    let library = tempfile::tempdir().expect("library temp dir");
    let source_path = external.path().join("晴天.flac");
    std::fs::write(&source_path, b"original-source-bytes").expect("write source file");
    let modified_before = std::fs::metadata(&source_path)
        .expect("stat source")
        .modified()
        .expect("mtime");

    let (root, fs) = real_adapter_over(library.path());

    // A user directory that happens to carry Echo's staging prefix.
    let foreign = library.path().join(".echo-staging-user-owned");
    std::fs::create_dir_all(&foreign).expect("foreign dir");
    std::fs::write(foreign.join("user-file.txt"), b"user-content").expect("foreign file");

    let (probe, metadata) = seeded_reader_fixtures();
    let database = MemoryDatabase::new();
    let deps = real_fs_deps(&fs, probe, metadata, &database);
    let sources = TempFileSources::new();
    sources.add("hit", "晴天.flac", &source_path);

    let report = PlanImport::new(&deps, &sources)
        .run(root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        song: _, target, ..
    } = &report.results[0]
    else {
        panic!("the import must succeed: {:?}", report.results[0]);
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");

    // 源文件内容/名称/位置不变: the source was copied, never moved.
    assert_eq!(
        std::fs::read(&source_path).expect("source bytes"),
        b"original-source-bytes"
    );
    assert!(
        source_path.is_file() && source_path.parent() == Some(external.path()),
        "the source keeps its name and location"
    );
    assert_eq!(
        std::fs::metadata(&source_path)
            .expect("stat source")
            .modified()
            .expect("mtime"),
        modified_before,
        "the source was never written to"
    );
    assert_eq!(
        sources.opened(),
        vec!["hit".to_owned()],
        "read exactly once"
    );

    // The full audio was published into the library and committed.
    assert_eq!(
        std::fs::read(library.path().join("media/歌手/歌手 - 晴天.flac")).expect("published"),
        b"original-source-bytes"
    );
    assert_eq!(database.songs().len(), 1);

    // 同名用户目录绝不被写入: the foreign directory keeps exactly its
    // original content; Echo staged in its own marker-verified directory.
    assert_eq!(
        std::fs::read(foreign.join("user-file.txt")).expect("user file"),
        b"user-content"
    );
    assert_eq!(std::fs::read_dir(&foreign).expect("read dir").count(), 1);
    // Echo staged inside its own portable control surface: `echo/tmp/` is
    // created and marker-owned, and `media/` gained the song. The foreign
    // legacy-prefix directory is never touched or adopted.
    assert!(
        library
            .path()
            .join("echo/tmp/.echo-ownership-marker")
            .is_file(),
        "the portable echo/tmp staging root carries its ownership marker"
    );
}

// -----------------------------------------------------------------------
// Task 5.4: same-named `.lrc` optional sub-resource with an independent
// result — embedded lyrics win once the audio is committed, a successful
// sidecar pairs with the audio's FINAL base name (including `(n)`), and a
// sidecar failure is "audio succeeded / lyrics failed" with no half
// sidecar left behind.
// -----------------------------------------------------------------------

/// Register embedded lyrics on the published audio path so the commit's
/// scan parse produces an Embedded candidate.
fn with_embedded_lyrics(g: &Gated, path: &str, raw: &str) {
    g.fixture.metadata.set(
        path,
        ParsedMetadata {
            artist: Some("歌手".to_owned()),
            title: Some("晴天".to_owned()),
            embedded_lyrics: Some(raw.to_owned()),
            ..ParsedMetadata::default()
        },
    );
}

/// The published sidecar bytes of the imported song (assertion helper).
fn read_library_file(g: &Gated, rel: &str) -> Vec<u8> {
    let base = g.fixture.fs.root_path(g.fixture.root).expect("root");
    std::fs::read(base.join(rel)).expect("published file")
}

#[test]
fn lrc_sidecar_is_copied_under_the_audio_target_and_result_reports_it() {
    let g = gated();
    g.sources.add("hit", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("hit", "晴天.lrc", b"[00:01.00]sidecar line");
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
        panic!("the audio input must import: {:?}", report.results[0]);
    };

    // The sidecar pairs with the audio's final base name.
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        &*lyrics,
        &LyricsImportResult::Imported {
            target: RelativeMediaPath::new("media/歌手/歌手 - 晴天.lrc").unwrap()
        },
        "the result reports the published sidecar"
    );
    assert_eq!(
        read_library_file(&g, "media/歌手/歌手 - 晴天.lrc"),
        b"[00:01.00]sidecar line",
        "the sidecar bytes landed at the paired target"
    );

    // The committed song carries a Sidecar candidate (the scan parse
    // picks the published `.lrc` up through the shared pipeline).
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&g.fixture.database, song)
            .unwrap();
    assert!(
        candidates
            .iter()
            .any(|c| c.source() == LyricsSource::Sidecar),
        "the sidecar candidate is persisted: {candidates:?}"
    );
    // No half sidecar, no empty one: only the one LC-published file.
    assert!(
        g.fixture.fs.staged_count() == 0,
        "both staged copies were published/discarded"
    );
}

#[test]
fn embedded_lyrics_still_win_over_the_imported_sidecar() {
    let g = gated();
    g.sources.add("hit", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("hit", "晴天.lrc", b"[00:01.00]sidecar line");
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
    // The published audio carries embedded lyrics (USLT/lyrics tag).
    with_embedded_lyrics(&g, "media/歌手/歌手 - 晴天.flac", "[00:01.00]embedded line");

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, .. } = report.results.into_iter().next().expect("one")
    else {
        panic!("the input must import");
    };

    // Both sources are stored; the effective one is ELSE EMBEDDED
    // (spec: 扫描后的歌曲仅在没有内嵌歌词时使用该侧车歌词).
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&g.fixture.database, song)
            .unwrap();
    let selected = select_effective_lyrics(&candidates).expect("effective lyrics");
    assert_eq!(
        selected.source(),
        LyricsSource::Embedded,
        "embedded lyrics win over the sidecar"
    );
    assert!(
        candidates
            .iter()
            .any(|c| c.source() == LyricsSource::Sidecar),
        "the sidecar is still stored as the fallback source"
    );
}

#[test]
fn lrc_target_pairs_with_a_numbered_audio_target() {
    let g = gated();
    // The base name is occupied by an existing record+file; the audio
    // lands on `(2)`, and the sidecar must follow the SAME final stem.
    g.fixture
        .write_file("media/歌手/歌手 - 晴天.flac", b"incumbent");
    seed_song(&g, "media/歌手/歌手 - 晴天.flac", b"incumbent");
    g.sources.add("new", "晴天.flac", b"audio-bytes");
    g.sources
        .add_sidecar("new", "晴天.lrc", b"[00:01.00]sidecar line");
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天 (2).flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("new")])
        .expect("batch-level success");
    let ImportOutcome::Imported {
        song,
        target,
        lyrics,
        ..
    } = report.results.first().cloned().expect("one result")
    else {
        panic!("the numbered input must import: {:?}", report.results[0]);
    };

    assert_eq!(target.display(), "media/歌手/歌手 - 晴天 (2).flac");
    assert_eq!(
        &*lyrics,
        &LyricsImportResult::Imported {
            target: RelativeMediaPath::new("media/歌手/歌手 - 晴天 (2).lrc").unwrap()
        },
        "the sidecar pairs with the FINAL numbered base name"
    );
    assert_eq!(
        read_library_file(&g, "media/歌手/歌手 - 晴天 (2).lrc"),
        b"[00:01.00]sidecar line"
    );
    // The incumbent pair survives untouched.
    assert_eq!(
        read_library_file(&g, "media/歌手/歌手 - 晴天.flac"),
        b"incumbent"
    );
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&g.fixture.database, song)
            .unwrap();
    assert!(
        candidates
            .iter()
            .any(|c| c.source() == LyricsSource::Sidecar),
        "the numbered sidecar is the song's fallback"
    );
}

#[test]
fn no_sidecar_means_no_lrc_file_and_none_result() {
    let g = gated();
    g.sources.add("bare", "晴天.flac", b"audio-bytes");
    tagged(&g, b"audio-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("bare")])
        .expect("batch-level success");
    let ImportOutcome::Imported { lyrics, .. } =
        report.results.into_iter().next().expect("one result")
    else {
        panic!("the bare input must import");
    };
    assert_eq!(
        &*lyrics,
        &LyricsImportResult::None,
        "no same-basename `.lrc` produces no sidecar result"
    );
    assert!(
        !g.fixture
            .fs
            .root_path(g.fixture.root)
            .unwrap()
            .join("media/歌手/歌手 - 晴天.lrc")
            .exists(),
        "没有同名 `.lrc` 时不得创建空歌词文件"
    );
    assert_eq!(g.sources.sidecar_read_keys(), Vec::<String>::new());
}
