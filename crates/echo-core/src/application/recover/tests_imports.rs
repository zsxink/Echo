//! Import/record-materialization crash recovery, watcher preemption and foreign-content conflicts.

#[allow(clippy::wildcard_imports)]
use super::tests::*;

#[test]
fn crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice() {
    let expect_success: Vec<(Site, Phase)> = vec![
        (Site::State("copy_pending"), Phase::Before),
        (Site::State("copy_applied"), Phase::Before),
        (Site::State("validate_pending"), Phase::Before),
        (Site::State("validated"), Phase::Before),
        (Site::State("publish_pending"), Phase::Before),
        (Site::State("publish_applied"), Phase::Before),
        // `database_committed` is written atomically with the song inside
        // the DB transaction, so a crash there is exactly a DB-commit
        // interruption (covered by `Commit Before/After` below).
        (Site::State("completed"), Phase::Before),
        (Site::Publish, Phase::Before),
        (Site::Publish, Phase::After),
        (Site::Commit, Phase::Before),
        (Site::Commit, Phase::After),
        // A crash after the DB commit but before/after the portable record
        // write (the new materialization side effect): recovery must
        // re-drive the record idempotently.
        (Site::RecordWrite, Phase::Before),
        (Site::RecordWrite, Phase::After),
    ];
    for (site, phase) in expect_success {
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(&fixture, &ctrl, Point { site, phase });
        assert!(ctrl.fired(), "the crash fired at {site:?} {phase:?}");
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);

        // First recovery converges to the unique terminal state.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_unique_terminal(&fixture, operation, reserved);

        // Second recovery is a no-op: same UUID, still one song, no
        // duplicate file/record, claims not released twice.
        let before = fixture.all_songs();
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        let after = fixture.all_songs();
        assert_eq!(
            before, after,
            "second recovery is idempotent at {site:?} {phase:?}"
        );
        assert_eq!(after.len(), 1, "still exactly one record");
        assert_eq!(
            after[0].id(),
            reserved,
            "same reserved UUID across recoveries"
        );
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }
}

#[test]
fn crash_during_record_materialization_converges_to_one_record_and_outbox() {
    // The record write happens AFTER the DB commit, so a crash there is a
    // distinctive window: the song row + outbox row are durable but the
    // `echo/records/songs/<prefix>/<uuid>.json` is not (Before) or was
    // only half-applied (After). Recovery's roll_forward must re-drive the
    // materialization idempotently — same revision, overwriting the same
    // record file — leaving exactly ONE SongRecord and ONE outbox row.
    for (site, phase) in [
        (Site::RecordWrite, Phase::Before),
        (Site::RecordWrite, Phase::After),
    ] {
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(&fixture, &ctrl, Point { site, phase });
        assert!(ctrl.fired(), "the crash fired at {site:?} {phase:?}");
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);

        // First recovery converges to the unique terminal state and writes
        // exactly one portable record + one outbox row.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_unique_terminal(&fixture, operation, reserved);
        let records = fixture.control.records_of(fixture.root);
        assert_eq!(
            records.len(),
            1,
            "exactly one portable record at {site:?} {phase:?}: {records:?}"
        );
        let PortableRecord::Song(song_record) = &records[0] else {
            panic!("the record is a song record");
        };
        assert_eq!(
            song_record.song_uuid,
            reserved.as_uuid(),
            "the record carries the journal-reserved identity"
        );
        assert_eq!(
            fixture.database.outbox_rows("song", &reserved.to_string()),
            vec![1],
            "exactly one sync-outbox row (revision 1)"
        );

        // Second recovery is a no-op: still one record, one outbox row.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(
            fixture.control.records_of(fixture.root).len(),
            1,
            "second recovery does not duplicate the record at {site:?} {phase:?}"
        );
        assert_eq!(
            fixture.database.outbox_rows("song", &reserved.to_string()),
            vec![1],
            "second recovery does not duplicate the outbox row"
        );
    }
}

#[test]
fn copy_crash_before_any_journal_leaves_nothing_to_recover() {
    // A crash during the streaming copy happens during pre-flight, before
    // the journal envelope/claim exists: nothing is recoverable, nothing
    // is orphaned, nothing enters the library.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    let deps = crash_deps(&fixture, &ctrl);
    let sources = FakeImportSources::new();
    wire_single(&fixture, &sources, "sunny", b"sunny-bytes");
    ctrl.arm(Point {
        site: Site::Copy,
        phase: Phase::Before,
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        PlanImport::new(&deps, &sources)
            .run(fixture.root, &[source("sunny")])
            .expect("batch-level success")
    }));
    assert!(outcome.is_err(), "the copy crash aborted the import");
    assert!(fixture.all_songs().is_empty(), "no record was ever created");
    let report = RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert!(report.touched.is_empty(), "no incomplete journal items");
    assert!(
        read_file(&fixture, "media/歌手/歌手 - 晴天.flac").is_none(),
        "no orphan final file"
    );
}

#[test]
fn truncated_source_is_rejected_and_leaves_nothing() {
    // The P2 "reading mid-copy truncation" gap: a source that serves fewer
    // bytes than it described must be rejected at validation and leave no
    // final file, no record, and a rolled-back journal.
    let fixture = ScanFixture::new();
    let sources = FakeImportSources::new();
    sources.add_truncated("broken", "晴天.flac", 200, b"only-partial");
    fixture.metadata.set_bytes(
        b"only-partial",
        ParsedMetadata {
            artist: Some("歌手".to_owned()),
            title: Some("晴天".to_owned()),
            ..ParsedMetadata::default()
        },
    );
    fixture.set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
    let report = PlanImport::new(&fixture.deps, &sources)
        .run(fixture.root, &[source("broken")])
        .expect("batch-level success");
    assert!(
        matches!(report.results[0], ImportOutcome::Failed { .. }),
        "truncated source must fail, not import: {:?}",
        report.results[0]
    );
    assert!(
        fixture.all_songs().is_empty(),
        "no record from a truncated copy"
    );
    assert!(
        read_file(&fixture, "media/歌手/歌手 - 晴天.flac").is_none(),
        "no final file from a truncated copy"
    );
}

#[test]
fn recovery_never_creates_a_duplicate_when_a_watcher_preempts() {
    // Watcher preemption: the audio is published and a watcher reconciles
    // it under the RESERVED UUID before the import's DB commit (task 4.9).
    // Recovery must reuse that identity, never a second record or UUID.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    run_import_until_crash(
        &fixture,
        &ctrl,
        Point {
            site: Site::State("publish_applied"),
            phase: Phase::Before,
        },
    );
    let operation = single_operation(&fixture);
    let reserved = reserved_of(&fixture, operation);
    let target = "media/歌手/歌手 - 晴天.flac";
    assert!(
        read_file(&fixture, target).is_some(),
        "audio already published"
    );

    // The watcher applied a record under the reserved identity (the same
    // path `apply_reserved` uses in `WatchCoordinator`).
    let mut song = crate::domain::entities::Song::new(
        reserved,
        fixture.root,
        RelativeMediaPath::new(target).unwrap(),
        crate::domain::ids::Revision::INITIAL,
    );
    song.apply_scan_facts(
        fixture.deps.hasher.hash_of_bytes(b"sunny-bytes"),
        b"sunny-bytes".len() as u64,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    crate::application::ports::SongRepository::upsert(&fixture.database, &song).unwrap();
    assert_eq!(
        fixture.all_songs().len(),
        1,
        "watcher created the reserved record"
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    let songs = fixture.all_songs();
    assert_eq!(
        songs.len(),
        1,
        "no duplicate record from watcher + recovery"
    );
    assert_eq!(songs[0].id(), reserved, "the reserved UUID is the only one");
    let items = fixture.database.items(operation).expect("items");
    assert!(items.iter().all(|item| item.state.is_terminal()));
    assert_eq!(fixture.database.released_claims(), vec![operation]);
}

#[test]
fn foreign_target_content_is_a_held_conflict_never_overwritten() {
    // A target already occupied by non-empty foreign content is never
    // published over: recovery holds the item FailedRecoverable, keeps the
    // claim, leaves the foreign file intact, and creates no record.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    run_import_until_crash(
        &fixture,
        &ctrl,
        Point {
            site: Site::Publish,
            phase: Phase::Before,
        },
    );
    let operation = single_operation(&fixture);
    let reserved = reserved_of(&fixture, operation);
    // A foreign file (different content) now sits at the target, and the
    // staged copy is lost — the worst collision recovery can face.
    let base = fixture.fs.root_path(fixture.root).expect("root");
    let items = fixture.database.items(operation).expect("items");
    for item in &items {
        if let Some(staging) = item.staging_path.as_ref() {
            let _ = std::fs::remove_file(base.join(staging.normalized()));
        }
    }
    std::fs::create_dir_all(base.join("media/歌手")).expect("mkdir artist");
    std::fs::write(base.join("media/歌手/歌手 - 晴天.flac"), b"foreign-content")
        .expect("write foreign");

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert!(fixture.all_songs().is_empty(), "no record, no second UUID");
    assert_eq!(
        read_file(&fixture, "media/歌手/歌手 - 晴天.flac"),
        Some(b"foreign-content".to_vec()),
        "the foreign file is never overwritten"
    );
    let items = fixture.database.items(operation).expect("items");
    assert!(
        items
            .iter()
            .all(|item| item.state == OperationState::FailedRecoverable),
        "conflict is held FailedRecoverable: {items:?}"
    );
    assert!(
        fixture.database.released_claims().is_empty(),
        "an unresolved conflict keeps its claim (never released)"
    );
    // Recovery is idempotent on the conflict too.
    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert_eq!(
        read_file(&fixture, "media/歌手/歌手 - 晴天.flac"),
        Some(b"foreign-content".to_vec()),
        "second recovery still never overwrites"
    );
    assert!(fixture.all_songs().is_empty());
    let _ = reserved;
}

#[test]
fn nothing_recoverable_rolls_back_and_releases_cleanly() {
    // A crash while only a claim exists (no staged copy, no target) rolls
    // back cleanly: nothing created, the item RolledBack, claim released.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    run_import_until_crash(
        &fixture,
        &ctrl,
        Point {
            site: Site::State("copy_pending"),
            phase: Phase::Before,
        },
    );
    let operation = single_operation(&fixture);
    let reserved = reserved_of(&fixture, operation);
    // Remove the staged copy and any target so nothing is recoverable.
    let base = fixture.fs.root_path(fixture.root).expect("root");
    for item in fixture.database.items(operation).expect("items") {
        if let Some(staging) = item.staging_path.as_ref() {
            let _ = std::fs::remove_file(base.join(staging.normalized()));
        }
    }

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert!(fixture.all_songs().is_empty(), "nothing created");
    let items = fixture.database.items(operation).expect("items");
    assert!(
        items
            .iter()
            .all(|item| item.state == OperationState::RolledBack),
        "nothing usable rolls back and releases: {items:?}"
    );
    assert_eq!(
        fixture.database.released_claims(),
        vec![operation],
        "the claim is released"
    );
    let _ = reserved;
    // Second recovery no-op (idempotent, no double release).
    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert_eq!(fixture.database.released_claims(), vec![operation]);
}

// -----------------------------------------------------------------------
// Recovery-matrix tests for Echo's own delete (task 5.7, design §9).
// Crash during stage / hide / restore, then recover TWICE to the unique
// hidden-or-restored terminal state; a mid-restore crash must still bring
// the song back to Available keeping UUID/favorite/stats/playlist position.
// -----------------------------------------------------------------------
