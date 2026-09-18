/// Register the tags an import source's *content* carries — the input the
/// task-5.2 naming step parses before planning the target.
fn tagged(g: &Gated, bytes: &[u8], artist: Option<&str>, title: Option<&str>) {
    g.fixture.metadata.set_bytes(
        bytes,
        ParsedMetadata {
            artist: artist.map(ToOwned::to_owned),
            title: title.map(ToOwned::to_owned),
            ..ParsedMetadata::default()
        },
    );
}

/// A fs wrapper that refuses every *external* side effect unless the
/// operation's journal reservation (`Planned` item with a reserved
/// `SongId`) is already persisted — the test double that proves
/// 先持久化意图，再执行调用 for the publish. Staging itself is internal
/// scratch inside Echo's marker-verified directory (invisible to the
/// library), so the gate guards the publication boundary.
struct ReservationGate {
    inner: FakeLibraryFileSystem,
    journal: MemoryDatabase,
    violations: Arc<Mutex<Vec<String>>>,
}

impl ReservationGate {
    fn verify_reserved(&self, operation: OperationId) {
        // The reserved identity + target claim must be durable before the
        // first external side effect. The import now advances each item
        // through the Copy/Validate/Publish chain, so before the publish
        // the item holds the reserved SongId in ANY pre-publish state
        // (task 5.5) — never just `Planned`.
        let reserved = OperationJournalRepository::items(&self.journal, operation)
            .unwrap_or_default()
            .into_iter()
            .any(|item| item.song.is_some() && !item.state.is_terminal());
        if !reserved {
            self.violations
                .lock()
                .unwrap()
                .push(format!("side effect before reservation: {operation}"));
        }
    }
}

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for ReservationGate {
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
        self.verify_reserved(staged.operation());
        self.inner.publish(root, staged, target)
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

/// The gated test composition: the use case sees the gate as its file
/// system, so every stage/publish proves the reservation ordering.
struct Gated {
    fixture: ScanFixture,
    gate: Arc<ReservationGate>,
    deps: ScanDeps,
    sources: FakeImportSources,
}

fn gated() -> Gated {
    let fixture = ScanFixture::new();
    let gate = Arc::new(ReservationGate {
        inner: fixture.fs.clone(),
        journal: fixture.database.clone(),
        violations: Arc::new(Mutex::new(Vec::new())),
    });
    let deps = {
        let fs: Arc<dyn LibraryFileSystem> = gate.clone();
        ScanDeps {
            fs,
            ..ScanDeps::clone(&fixture.deps)
        }
    };
    Gated {
        fixture,
        gate,
        deps,
        sources: FakeImportSources::new(),
    }
}

fn source(key: &str) -> ImportSource {
    ImportSource::new(key).expect("valid source handle")
}

fn seed_song(gated: &Gated, path: &str, bytes: &[u8]) -> Song {
    let mut song = Song::new(
        crate::domain::ids::SongId::new(),
        gated.fixture.root,
        RelativeMediaPath::new(path).expect("valid path"),
        Revision::INITIAL,
    );
    song.apply_scan_facts(
        gated.deps.hasher.hash_of_bytes(bytes),
        u64::try_from(bytes.len()).unwrap_or(1),
        1,
        AudioFormat::Flac,
    );
    gated.fixture.database.upsert(&song).expect("seed song");
    song
}

fn violations(gated: &Gated) -> Vec<String> {
    gated.gate.violations.lock().unwrap().clone()
}

/// Register the tags an import source's *content* carries on a bare
/// fixture (the content-keyed metadata lookup the planning step uses).
fn tag_content(fixture: &ScanFixture, bytes: &[u8], artist: Option<&str>, title: Option<&str>) {
    fixture.metadata.set_bytes(
        bytes,
        ParsedMetadata {
            artist: artist.map(ToOwned::to_owned),
            title: title.map(ToOwned::to_owned),
            ..ParsedMetadata::default()
        },
    );
}

/// The operation id of the first imported input (assertion helper).
fn operation_of(report: &ImportBatchReport) -> Option<OperationId> {
    report.results.iter().find_map(|outcome| match outcome {
        ImportOutcome::Imported { operation, .. } => Some(*operation),
        _ => None,
    })
}

/// An [`ImportSourceReader`] over REAL files inside injected temp
/// directories — lets the import prove source invariance end-to-end
/// without ever touching the user's home.
struct TempFileSources {
    files: Mutex<BTreeMap<String, (String, std::path::PathBuf)>>,
    sidecars: Mutex<BTreeMap<String, (String, std::path::PathBuf)>>,
    opened: Mutex<Vec<String>>,
}

impl TempFileSources {
    fn new() -> Self {
        Self {
            files: Mutex::new(BTreeMap::new()),
            sidecars: Mutex::new(BTreeMap::new()),
            opened: Mutex::new(Vec::new()),
        }
    }
    fn add(&self, key: &str, display_name: &str, path: &std::path::Path) {
        self.files.lock().unwrap().insert(
            key.to_owned(),
            (display_name.to_owned(), path.to_path_buf()),
        );
    }
    fn add_sidecar(&self, key: &str, display_name: &str, path: &std::path::Path) {
        self.sidecars.lock().unwrap().insert(
            key.to_owned(),
            (display_name.to_owned(), path.to_path_buf()),
        );
    }
    fn opened(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
    }
}

impl ImportSourceReader for TempFileSources {
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error> {
        let (display_name, path) = {
            let map = self.files.lock().unwrap();
            match map.get(source.key()) {
                Some(entry) => entry.clone(),
                None => return Err(Error::unavailable("import source", "unknown handle")),
            }
        };
        let size = std::fs::metadata(&path)
            .map_err(|e| Error::io("stat import source", e, path.clone()))?
            .len();
        Ok(ImportSourceInfo { display_name, size })
    }

    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error> {
        self.opened.lock().unwrap().push(source.key().to_owned());
        let path = {
            let map = self.files.lock().unwrap();
            match map.get(source.key()) {
                Some((_, path)) => path.clone(),
                None => return Err(Error::unavailable("import source", "unknown handle")),
            }
        };
        let file = std::fs::File::open(&path)
            .map_err(|e| Error::io("open import source", e, path.clone()))?;
        Ok(Box::new(file))
    }

    fn sidecar(&self, source: &ImportSource) -> Result<Option<SidecarInfo>, Error> {
        let (name, path) = {
            let map = self.sidecars.lock().unwrap();
            match map.get(source.key()) {
                Some(entry) => entry.clone(),
                None => return Ok(None),
            }
        };
        let size = std::fs::metadata(&path)
            .map_err(|e| Error::io("stat import sidecar", e, path.clone()))?
            .len();
        Ok(Some(SidecarInfo {
            display_name: name,
            size,
        }))
    }

    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn Read + 'a>>, Error> {
        let path = {
            let map = self.sidecars.lock().unwrap();
            match map.get(source.key()) {
                Some((_, path)) => path.clone(),
                None => return Ok(None),
            }
        };
        let file = std::fs::File::open(&path)
            .map_err(|e| Error::io("open import sidecar", e, path.clone()))?;
        Ok(Some(Box::new(file)))
    }
}

/// A fs wrapper that snapshots the song table at every ingestion step —
/// the test double proving the record exists only after the full audio
/// publish (`DatabaseCommitted` comes after `publish`).
struct VisibilityProbe {
    inner: FakeLibraryFileSystem,
    songs: MemoryDatabase,
    root: LibraryRootId,
    events: Mutex<Vec<(&'static str, usize)>>,
}

impl VisibilityProbe {
    fn record(&self, event: &'static str) {
        let count =
            crate::application::ports::SongRepository::all_in_root(&self.songs, self.root)
                .map_or(usize::MAX, |songs| songs.len());
        self.events.lock().unwrap().push((event, count));
    }
    fn events(&self) -> Vec<(&'static str, usize)> {
        self.events.lock().unwrap().clone()
    }
}

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for VisibilityProbe {
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
        let result = self.inner.publish(root, staged, target);
        self.record("publish");
        result
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
        let result = self.inner.stage_stream(root, staged, content);
        self.record("stage_stream");
        result
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
fn mixed_batch_reports_each_input_independently() {
    let g = gated();
    // An existing library record owning some content, for the duplicate.
    let existing = seed_song(&g, "media/已有/other.flac", b"library-original");
    // The successful input: a supported audio file whose tags name the
    // target 歌手/歌手 - 晴天.flac and whose published file parses cleanly.
    g.sources.add("good", "晴天.flac", b"sunny-bytes");
    tagged(&g, b"sunny-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
    g.sources.add("dup", "重复.flac", b"library-original");
    g.sources.add("text", "notes.txt", b"not audio");
    g.sources.add("locked", "locked.flac", b"unreadable");
    g.sources.fail("locked", "选择的外部文件不可读");

    let batch = [
        source("good"),
        source("dup"),
        source("text"),
        source("locked"),
    ];
    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &batch)
        .expect("batch-level success");

    assert_eq!(report.results.len(), batch.len(), "one result per input");
    let ImportOutcome::Imported {
        operation: _,
        song: imported,
        target,
        ..
    } = &report.results[0]
    else {
        panic!(
            "the supported audio input must import: {:?}",
            report.results[0]
        );
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        report.results[1],
        ImportOutcome::Duplicate {
            existing: existing.id()
        },
        "library content duplicate returns the existing UUID"
    );
    assert_eq!(
        report.results[2],
        ImportOutcome::Skipped,
        "a non-audio input in a multi-select is a benign normal-skip"
    );
    let ImportOutcome::Failed { code, .. } = &report.results[3] else {
        panic!(
            "the unreadable source must fail, not abort: {:?}",
            report.results[3]
        );
    };
    assert_eq!(*code, "permission");

    // The imported input is fully usable: the file is published with the
    // exact source bytes and the record exists under the reserved identity.
    let published = g
        .fixture
        .fs
        .root_path(g.fixture.root)
        .expect("root")
        .join("media/歌手/歌手 - 晴天.flac");
    assert_eq!(
        std::fs::read(&published).expect("published bytes"),
        b"sunny-bytes"
    );
    let songs = g.fixture.all_songs();
    assert_eq!(songs.len(), 2, "seed + import, nothing else");
    let record = songs
        .iter()
        .find(|song| song.id() == *imported)
        .expect("record");
    assert_eq!(record.title(), Some("晴天"));
    assert_eq!(
        record.path(),
        &RelativeMediaPath::new("media/歌手/歌手 - 晴天.flac").unwrap()
    );
    assert!(violations(&g).is_empty());
}

#[test]
fn import_reserves_operation_and_song_ids_before_side_effects() {
    let g = gated();
    g.sources.add("good", "晴天.flac", b"reserved-bytes");
    tagged(&g, b"reserved-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);

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
        panic!("the supported input must import");
    };

    assert!(
        violations(&g).is_empty(),
        "every stage/publish ran after the journal reservation: {:?}",
        violations(&g)
    );
    // The journal carries the operation with the RESERVED identity, the
    // target claim and the expected BLAKE3.
    let items = g.deps.journal.items(operation).expect("journal items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].state, OperationState::Completed);
    assert_eq!(items[0].kind, OperationResourceKind::Audio);
    assert_eq!(items[0].song, Some(song), "UUID equals the reservation");
    assert_eq!(items[0].target_path, target);
    assert_eq!(items[0].claim_key, target.identity_key());
    assert_eq!(
        items[0].expected_hash,
        g.deps.hasher.hash_of_bytes(b"reserved-bytes"),
        "the journal records the expected content hash"
    );
    // Terminal state releases the claim (port contract).
    assert_eq!(g.fixture.database.released_claims(), vec![operation]);
    // The committed record carries the reserved UUID.
    let record = g.deps.songs.by_id(song).expect("query").expect("committed");
    assert_eq!(record.id(), song);
    assert_eq!(record.path(), &target);
}
