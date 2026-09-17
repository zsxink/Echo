#[test]
fn single_input_failure_does_not_rollback_committed_inputs() {
    let g = gated();
    g.sources.add("a", "a.flac", b"content-a");
    tagged(&g, b"content-a", Some("歌手"), Some("A"));
    g.fixture.set_audio("media/歌手/歌手 - A.flac", "A", 1_000);
    g.sources.fail("b", "外部文件不可读");
    g.sources.add("c", "c.flac", b"content-c");
    tagged(&g, b"content-c", Some("歌手"), Some("C"));
    g.fixture.set_audio("media/歌手/歌手 - C.flac", "C", 3_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("a"), source("b"), source("c")])
        .expect("batch-level success");

    let ImportOutcome::Imported { song: a, .. } = report.results[0] else {
        panic!("the first input must succeed");
    };
    assert!(matches!(report.results[1], ImportOutcome::Failed { .. }));
    let ImportOutcome::Imported { song: c, .. } = report.results[2] else {
        panic!("the input AFTER the failure must still succeed");
    };
    assert_ne!(a, c, "each input reserves its own UUID");
    assert_eq!(
        g.fixture.all_songs().len(),
        2,
        "both successful inputs committed"
    );
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert!(root_dir.join("media/歌手/歌手 - A.flac").exists());
    assert!(root_dir.join("media/歌手/歌手 - C.flac").exists());
    // The failed input left no claim behind: retries start clean.
    assert_eq!(g.fixture.database.released_claims().len(), 2);
    assert!(violations(&g).is_empty());
}

#[test]
fn duplicate_content_within_a_batch_creates_one_record() {
    let g = gated();
    g.sources.add("x", "tune.flac", b"same-content");
    tagged(&g, b"same-content", Some("歌手"), Some("Tune"));
    g.fixture
        .set_audio("media/歌手/歌手 - Tune.flac", "Tune", 1_000);
    g.sources.add("y", "copy.flac", b"same-content");

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("x"), source("y")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song: first, .. } = report.results[0] else {
        panic!("the first input must import");
    };
    assert_eq!(
        report.results[1],
        ImportOutcome::Duplicate { existing: first },
        "the second copy of the same content reports the first import"
    );
    assert_eq!(g.fixture.all_songs().len(), 1, "one record per content");
}

#[test]
fn unavailable_root_rejects_the_whole_batch_before_copying() {
    let g = gated();
    g.sources.add("good", "晴天.flac", b"some-bytes");
    g.sources.add("other", "other.flac", b"other-bytes");
    g.fixture.fs.set_write_capable(false);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("good"), source("other")])
        .expect("batch-level success");

    assert_eq!(
        report.results,
        vec![
            ImportOutcome::LibraryUnavailable,
            ImportOutcome::LibraryUnavailable
        ],
        "every input reports the library as unavailable"
    );
    assert!(
        g.sources.read_keys().is_empty(),
        "no source is read before the batch is refused"
    );
    assert!(
        g.fixture.all_songs().is_empty(),
        "nothing entered the library"
    );
}

#[test]
fn same_name_different_content_gets_minimal_conflict_number() {
    let g = gated();
    g.fixture
        .write_file("media/歌手/歌手 - 晴天.flac", b"original-content");
    seed_song(&g, "media/歌手/歌手 - 晴天.flac", b"original-content");
    g.sources.add("new", "晴天.flac", b"different-content");
    tagged(&g, b"different-content", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天 (2).flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("new")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, target, .. } =
        report.results.into_iter().next().expect("one")
    else {
        panic!("the import must succeed under a numbered name");
    };
    assert_eq!(
        target.display(),
        "media/歌手/歌手 - 晴天 (2).flac",
        "minimal (n) after the occupied name"
    );

    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("original"),
        b"original-content",
        "the existing file is never replaced"
    );
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天 (2).flac")).expect("new"),
        b"different-content"
    );
    assert_eq!(g.fixture.all_songs().len(), 2);
    let record = g.deps.songs.by_id(song).expect("query").expect("record");
    assert_eq!(
        record.path(),
        &RelativeMediaPath::new("media/歌手/歌手 - 晴天 (2).flac").unwrap()
    );
}

// -----------------------------------------------------------------------
// Task 5.2: default naming, fallbacks, platform cleanup, truncation,
// minimal numbering and the never-overwrite guarantee.
// -----------------------------------------------------------------------

#[test]
fn default_target_is_artist_folder_with_artist_minus_title() {
    let g = gated();
    // The original extension case is normalized to lowercase for the
    // deterministic target name (`.FLAC` → `.flac`).
    g.sources.add("hit", "晴天.FLAC", b"hit-bytes");
    tagged(&g, b"hit-bytes", Some("周杰伦"), Some("晴天"));
    g.fixture
        .set_audio("media/周杰伦/周杰伦 - 晴天.flac", "晴天", 269_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("hit")])
        .expect("batch-level success");
    let ImportOutcome::Imported { song, target, .. } =
        report.results.into_iter().next().expect("one result")
    else {
        panic!("the tagged input must import");
    };

    assert_eq!(
        target.display(),
        "media/周杰伦/周杰伦 - 晴天.flac",
        "target is 歌手/歌手 - 歌曲名.扩展名"
    );
    let published = g
        .fixture
        .fs
        .root_path(g.fixture.root)
        .expect("root")
        .join(target.normalized());
    assert_eq!(
        std::fs::read(&published).expect("published bytes"),
        b"hit-bytes"
    );
    let record = g.deps.songs.by_id(song).expect("query").expect("record");
    assert_eq!(record.path(), &target);
    assert_eq!(record.title(), Some("晴天"));
}

#[test]
fn missing_or_blank_tags_fall_back_to_deterministic_names() {
    let g = gated();
    // No tags at all → 未知艺人/未知艺人 - 未命名歌曲.flac.
    g.sources.add("none", "mystery.flac", b"bytes-none");
    g.fixture.set_audio(
        "media/未知艺人/未知艺人 - 未命名歌曲.flac",
        "未命名歌曲",
        1_000,
    );
    // Blank (whitespace-only) tags count as missing → same fallback, so
    // this input collides with the first and takes the minimal (2).
    g.sources.add("blank", "blank.flac", b"bytes-blank");
    tagged(&g, b"bytes-blank", Some("   "), Some("\u{3000}"));
    g.fixture.set_audio(
        "media/未知艺人/未知艺人 - 未命名歌曲 (2).flac",
        "未命名歌曲",
        2_000,
    );
    // Title only → the artist still takes 未知艺人.
    g.sources.add("title-only", "t.flac", b"bytes-title");
    tagged(&g, b"bytes-title", Some("\t"), Some("晴天"));
    g.fixture
        .set_audio("media/未知艺人/未知艺人 - 晴天.flac", "晴天", 3_000);
    // Artist only → the title still takes 未命名歌曲.
    g.sources.add("artist-only", "a.flac", b"bytes-artist");
    tagged(&g, b"bytes-artist", Some("周杰伦"), None);
    g.fixture
        .set_audio("media/周杰伦/周杰伦 - 未命名歌曲.flac", "未命名歌曲", 4_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(
            g.fixture.root,
            &[
                source("none"),
                source("blank"),
                source("title-only"),
                source("artist-only"),
            ],
        )
        .expect("batch-level success");

    let targets: Vec<&str> = report
        .results
        .iter()
        .map(|outcome| match outcome {
            ImportOutcome::Imported { target, .. } => target.display(),
            other => panic!("every fallback input must import: {other:?}"),
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            "media/未知艺人/未知艺人 - 未命名歌曲.flac",
            "media/未知艺人/未知艺人 - 未命名歌曲 (2).flac",
            "media/未知艺人/未知艺人 - 晴天.flac",
            "media/周杰伦/周杰伦 - 未命名歌曲.flac",
        ]
    );
    assert_eq!(g.fixture.all_songs().len(), 4);
}

#[test]
fn tag_text_is_cleaned_for_platform_safe_targets() {
    let g = gated();
    // Forbidden separators/wildcards in tags, plus a control character
    // (dropped, not replaced) — cleaned on every platform's behalf.
    g.sources.add("messy", "track.flac", b"bytes-messy");
    tagged(&g, b"bytes-messy", Some("AC/DC"), Some("问\u{1}春*归?"));
    g.fixture
        .set_audio("media/AC_DC/AC_DC - 问春_归_.flac", "问春归", 1_000);
    // A Windows reserved device name as the artist.
    g.sources.add("con", "demo.flac", b"bytes-con");
    tagged(&g, b"bytes-con", Some("CON"), Some("Demo"));
    g.fixture
        .set_audio("media/CON_/CON_ - Demo.flac", "Demo", 2_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("messy"), source("con")])
        .expect("batch-level success");

    let targets: Vec<&str> = report
        .results
        .iter()
        .map(|outcome| match outcome {
            ImportOutcome::Imported { target, .. } => target.display(),
            other => panic!("every cleaned input must import: {other:?}"),
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            "media/AC_DC/AC_DC - 问春_归_.flac",
            "media/CON_/CON_ - Demo.flac"
        ]
    );
    // No platform-forbidden character survives inside any component
    // (the two `/` per target are the `media/` prefix and the separator
    // the builder itself emits).
    for target in &targets {
        let components: Vec<&str> = target.split('/').collect();
        assert_eq!(components.len(), 3, "media/artist/file form: {target}");
        assert_eq!(
            components[0], "media",
            "portable media tree prefix: {target}"
        );
        for component in &components[1..] {
            for c in ['\\', ':', '*', '?', '"', '<', '>', '|'] {
                assert!(!component.contains(c), "{c:?} survived in {target}");
            }
        }
    }
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert!(root_dir.join("media/AC_DC/AC_DC - 问春_归_.flac").exists());
    assert!(root_dir.join("media/CON_/CON_ - Demo.flac").exists());
}

#[test]
fn oversized_components_are_truncated_with_short_hash_and_keep_extension() {
    let g = gated();
    // A far-over-cap title (CJK: 3 bytes per char).
    let long_title = "这是一段非常长的歌曲标题".repeat(12);
    let expected_title_target = format!(
        "{MEDIA_ROOT}/{}/{}",
        target_artist_component(Some("周杰伦"), TARGET_COMPONENT_BYTES),
        truncate_component_with_extension(
            &target_file_stem(Some("周杰伦"), Some(&long_title)),
            "flac",
            TARGET_COMPONENT_BYTES
        )
    );
    g.sources.add("long", "long.flac", b"bytes-long");
    tagged(&g, b"bytes-long", Some("周杰伦"), Some(&long_title));
    g.fixture.set_audio(&expected_title_target, "长标题", 1_000);
    // A far-over-cap artist: the directory component is bounded too.
    let long_artist = "很长的艺人名字组合".repeat(30);
    let expected_artist_target = format!(
        "{MEDIA_ROOT}/{}/{}",
        target_artist_component(Some(&long_artist), TARGET_COMPONENT_BYTES),
        truncate_component_with_extension(
            &target_file_stem(Some(&long_artist), Some("短")),
            "flac",
            TARGET_COMPONENT_BYTES
        )
    );
    g.sources.add("long-artist", "long2.flac", b"bytes-la");
    tagged(&g, b"bytes-la", Some(&long_artist), Some("短"));
    g.fixture.set_audio(&expected_artist_target, "短", 2_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("long"), source("long-artist")])
        .expect("batch-level success");

    let targets: Vec<&str> = report
        .results
        .iter()
        .map(|outcome| match outcome {
            ImportOutcome::Imported { target, .. } => target.display(),
            other => panic!("the truncated input must import: {other:?}"),
        })
        .collect();
    assert_eq!(
        targets,
        vec![expected_title_target, expected_artist_target],
        "the target follows the domain truncation rule exactly"
    );
    for target in &targets {
        let (tree, rest) = target.split_once('/').expect("media/artist/file form");
        assert_eq!(tree, "media", "portable media tree prefix: {target}");
        let (dir, file) = rest.rsplit_once('/').expect("artist/file form");
        assert!(dir.len() <= TARGET_COMPONENT_BYTES, "{dir}");
        assert!(file.len() <= TARGET_COMPONENT_BYTES, "{file}");
        assert!(
            std::path::Path::new(file)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("flac")),
            "extension survives: {file}"
        );
        assert!(file.contains('~'), "short-hash suffix present: {file}");
    }
    // The two truncations stay distinguishable.
    assert_ne!(targets[0], targets[1]);
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert!(root_dir.join(targets[0]).exists());
    assert!(root_dir.join(targets[1]).exists());
}

#[test]
fn minimal_numbering_skips_occupied_suffixes() {
    let g = gated();
    // Both the base name and its first numbered successor are taken
    // (library record + on-disk file each); the import must land on (3).
    g.fixture
        .write_file("media/歌手/歌手 - 晴天.flac", b"content-1");
    seed_song(&g, "media/歌手/歌手 - 晴天.flac", b"content-1");
    g.fixture
        .write_file("media/歌手/歌手 - 晴天 (2).flac", b"content-2");
    seed_song(&g, "media/歌手/歌手 - 晴天 (2).flac", b"content-2");
    g.sources.add("new", "晴天.flac", b"content-3");
    tagged(&g, b"content-3", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天 (3).flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("new")])
        .expect("batch-level success");
    let ImportOutcome::Imported { target, .. } =
        report.results.into_iter().next().expect("one")
    else {
        panic!("the import must succeed under a numbered name");
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天 (3).flac");
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("first"),
        b"content-1",
        "occupied names are never replaced"
    );
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天 (2).flac")).expect("second"),
        b"content-2"
    );
    assert_eq!(g.fixture.all_songs().len(), 3);
}

#[test]
fn numbering_applies_within_one_batch() {
    let g = gated();
    // Same tags, different content, one batch: the second input must not
    // claim the first input's freshly planned target.
    g.sources.add("first", "one.flac", b"content-1");
    tagged(&g, b"content-1", Some("歌手"), Some("同名"));
    g.fixture
        .set_audio("media/歌手/歌手 - 同名.flac", "同名", 1_000);
    g.sources.add("second", "two.flac", b"content-2");
    tagged(&g, b"content-2", Some("歌手"), Some("同名"));
    g.fixture
        .set_audio("media/歌手/歌手 - 同名 (2).flac", "同名", 2_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("first"), source("second")])
        .expect("batch-level success");
    let targets: Vec<&str> = report
        .results
        .iter()
        .map(|outcome| match outcome {
            ImportOutcome::Imported { target, .. } => target.display(),
            other => panic!("both inputs must import: {other:?}"),
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            "media/歌手/歌手 - 同名.flac",
            "media/歌手/歌手 - 同名 (2).flac"
        ]
    );
    assert_eq!(g.fixture.all_songs().len(), 2, "two records, two files");
}

#[test]
fn import_renumbers_around_files_that_exist_only_on_disk() {
    let g = gated();
    // A file at the exact tag-derived target with NO library record
    // (external placement, or a database that was dropped): planning must
    // still not claim the name.
    g.fixture
        .write_file("media/歌手/歌手 - 晴天.flac", b"someone-else");
    g.sources.add("new", "晴天.flac", b"fresh-bytes");
    tagged(&g, b"fresh-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天 (2).flac", "晴天", 1_000);

    let report = PlanImport::new(&g.deps, &g.sources)
        .run(g.fixture.root, &[source("new")])
        .expect("batch-level success");
    let ImportOutcome::Imported { target, .. } =
        report.results.into_iter().next().expect("one")
    else {
        panic!("the import must succeed under a numbered name");
    };
    assert_eq!(target.display(), "media/歌手/歌手 - 晴天 (2).flac");
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("existing"),
        b"someone-else",
        "the disk-only file keeps its bytes"
    );
    assert_eq!(g.fixture.all_songs().len(), 1, "only the import");
}

/// A fs wrapper that plants a file at a chosen target during the
/// `stage_stream` — the watcher/other-process race window between the
/// batch snapshot and the publish. The port's create-new contract must
/// absorb it: the import fails, the existing file survives.
struct RacePlanter {
    inner: FakeLibraryFileSystem,
    plant: std::sync::Mutex<Option<String>>,
}

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for RacePlanter {
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
        let planted = self.plant.lock().unwrap().take();
        if let Some(rel) = planted {
            let base = self.inner.root_path(root).expect("root");
            let dest = base.join(&rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).expect("plant mkdir");
            }
            std::fs::write(&dest, b"planted-first").expect("plant write");
        }
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
fn publish_never_overwrites_a_target_created_after_planning() {
    let g = gated();
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
    g.sources.add("new", "晴天.flac", b"new-bytes");
    tagged(&g, b"new-bytes", Some("歌手"), Some("晴天"));
    g.fixture
        .set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 1_000);

    let report = PlanImport::new(&deps, &g.sources)
        .run(g.fixture.root, &[source("new")])
        .expect("batch-level success");
    let ImportOutcome::Failed { code, .. } = report.results.into_iter().next().expect("one")
    else {
        panic!("the raced import must fail, never overwrite");
    };
    assert_eq!(code, "conflict", "publish is create-new");

    // The planted file keeps its bytes; nothing entered the library; the
    // operation's claim was released for a clean retry.
    let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
    assert_eq!(
        std::fs::read(root_dir.join("media/歌手/歌手 - 晴天.flac")).expect("planted"),
        b"planted-first"
    );
    assert!(g.fixture.all_songs().is_empty());
    assert_eq!(g.fixture.database.released_claims().len(), 1);
}
