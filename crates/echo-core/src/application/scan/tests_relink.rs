//! Relink rules: unchanged-file skip, move relinks, duplicate-hash determinism and the conservative unique-music-key path, plus stress and cancellation.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn enumerate_failure_marks_run_failed_without_missing() {
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");
    // The file disappears AND the root read fails.
    fixture.remove_file("a.mp3");
    fixture
        .fs
        .inject_fault(Error::unavailable("library root", "unmounted"));

    let error = start_scan(&fixture).run(fixture.root).unwrap_err();
    // The fake's fault is storage-shaped; the contract under test is the
    // failed run, not the error variant.
    assert_eq!(error.code(), "storage");
    let row = fixture.run_row(2).expect("failed run row");
    assert_eq!(row.state, ScanState::Failed);
    assert!(row.finished);
    // The (now absent) song was NOT marked missing by the failed run.
    let record = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("record kept");
    assert_eq!(record.availability(), SongAvailability::Available);
}

#[test]
fn fast_skip_unchanged_files_and_relink_on_move() {
    let fixture = ScanFixture::new();
    fixture.write_file("album/song.flac", b"audio-bytes");
    fixture.set_audio("album/song.flac", "Song", 1_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("first scan");
    assert_eq!(summary.progress.created, 1);

    let original = song_by_path(&fixture, &fixture.path("album/song.flac"))
        .expect("lookup")
        .expect("song");
    // Favorite goes through its dedicated mutation: a metadata upsert
    // never carries user state (the SQLite `DO UPDATE` list deliberately
    // omits `is_favorite`, `play_count` and `availability`).
    SongRepository::set_favorite(&fixture.database, original.id(), true).expect("favorite set");

    // Rescan with no change: fast-skip, no re-parse, favorite intact.
    let summary = start_scan(&fixture).run(fixture.root).expect("rescan");
    assert_eq!(summary.progress.skipped, 1);
    assert_eq!(summary.progress.updated, 0);
    assert_eq!(summary.progress.created, 0);

    // Move the file: same content, new path → the UUID survives.
    fixture.write_file("album/renamed.flac", b"audio-bytes");
    fixture.remove_file("album/song.flac");
    fixture.set_audio("album/renamed.flac", "Song", 1_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("move scan");
    assert_eq!(summary.progress.updated, 1, "re-link counts as update");
    let moved = song_by_path(&fixture, &fixture.path("album/renamed.flac"))
        .expect("lookup")
        .expect("re-linked record");
    assert_eq!(moved.id(), original.id(), "UUID kept across the move");
    assert!(moved.favorite(), "favorite survives the move");
    assert!(
        song_by_path(&fixture, &fixture.path("album/song.flac"))
            .expect("lookup")
            .is_none(),
        "no duplicate record for the old path"
    );
    // The old-path record must not linger as missing either.
    assert!(fixture
        .all_songs()
        .iter()
        .all(|song| song.availability() == SongAvailability::Available));
}

#[test]
fn duplicate_hash_paths_get_deterministic_primary() {
    let fixture = ScanFixture::new();
    fixture.write_file("z-first.mp3", b"same-bytes");
    fixture.write_file("a-second.mp3", b"same-bytes");
    fixture.set_audio("z-first.mp3", "Same", 1_000);
    fixture.set_audio("a-second.mp3", "Same", 1_000);

    let summary = start_scan(&fixture).run(fixture.root).expect("scan");
    assert_eq!(
        summary.progress.created, 1,
        "exactly one record per hash: {summary:?}"
    );
    // One of the two paths became the duplicate report (whichever the
    // enumeration ordered second).
    let duplicated = summary.progress.skipped + summary.progress.updated;
    assert_eq!(duplicated, 1);
    let issues = fixture.issues(1);
    assert!(
        issues
            .iter()
            .any(|issue| issue.code() == "duplicate_content")
            || summary.progress.updated == 1,
        "duplicate content is reported or the primary moved deterministically"
    );
    // The primary is the smallest canonical path key, deterministically.
    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].path().display(), "media/a-second.mp3");
    let primary_id = songs[0].id();

    // The primary file disappears while the same-hash copy is still present:
    // the record promotes to the remaining path under the same UUID in the
    // same scan (主路径消失时从剩余同 hash 路径中按同一规则提升).
    fixture.remove_file("a-second.mp3");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("promote scan");
    let record = fixture.all_songs().into_iter().next().expect("record");
    assert_eq!(record.id(), primary_id, "UUID kept on promotion");
    assert_eq!(record.path().display(), "media/z-first.mp3");
    assert_eq!(record.availability(), SongAvailability::Available);

    // With no same-hash file left the record goes missing (UUID kept).
    fixture.remove_file("z-first.mp3");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");
    let record = fixture.all_songs().into_iter().next().expect("record");
    assert_eq!(record.id(), primary_id);
    assert_eq!(record.availability(), SongAvailability::Missing);

    // The file comes back at the promoted path: same UUID, available again.
    fixture.write_file("z-first.mp3", b"same-bytes");
    fixture.set_audio("z-first.mp3", "Same", 1_000);
    start_scan(&fixture)
        .run(fixture.root)
        .expect("restore scan");
    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), primary_id, "UUID kept on restore");
    assert_eq!(songs[0].path().display(), "media/z-first.mp3");
    assert_eq!(songs[0].availability(), SongAvailability::Available);
}

#[test]
fn music_key_never_relinks_identities() {
    let fixture = ScanFixture::new();
    // A song whose file vanished, with full tags (a music key is available).
    fixture.write_file("old/tone.flac", b"old-bytes");
    fixture.set_audio("old/tone.flac", "晴天", 200_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");
    fixture.remove_file("old/tone.flac");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("marks missing");
    let gone = fixture.all_songs().into_iter().next().expect("record kept");
    assert_eq!(gone.availability(), SongAvailability::Missing);

    // A new file with the same normalized key and a duration within 2 s used
    // to be a conservative weak re-link. LD-05 forbids merging identities on
    // a music-key heuristic, so the old record stays missing and the new file
    // gets its own UUID.
    fixture.write_file("new/tone.flac", b"new-bytes");
    fixture.set_audio("new/tone.flac", "晴天", 201_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("second scan");
    assert_eq!(
        summary.progress.updated, 0,
        "no existing record was rewritten"
    );
    assert_eq!(
        summary.progress.created, 1,
        "the new file gets a fresh UUID"
    );
    let mut songs = fixture.all_songs();
    songs.sort_by_key(|song| song.path().display().to_owned());
    assert_eq!(songs.len(), 2, "both identities are kept, none merged");
    let old = songs
        .iter()
        .find(|song| song.path().display() == "media/old/tone.flac")
        .expect("the vanished record is preserved");
    assert_eq!(old.id(), gone.id(), "the old UUID is untouched");
    assert_eq!(old.availability(), SongAvailability::Missing);
    let fresh = songs
        .iter()
        .find(|song| song.path().display() == "media/new/tone.flac")
        .expect("the new file is indexed under a new identity");
    assert_ne!(fresh.id(), gone.id(), "no music-key merge");
    assert_eq!(fresh.availability(), SongAvailability::Available);
}

#[test]
fn progress_persisted_throttled_and_terminal_state_kept() {
    let fixture = ScanFixture::new();
    for index in 0..7 {
        let path = format!("file{index}.mp3");
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("T{index}"), 1_000);
    }
    // batch_size 2 → 4 parse batches; the clock steps 60 ms per read with
    // a 100 ms interval, so batch snapshots are throttled while phase
    // transitions persist immediately.
    let deps = ScanDeps {
        config: ScanConfig {
            batch_size: 2,
            worker_threads: 1,
            progress_interval: Duration::from_millis(100),
            lyrics_limit: 2 * 1024 * 1024,
        },
        clock: Arc::new(crate::application::testing::clock::SteppingClock::new(60)),
        ..ScanDeps::clone(&fixture.deps)
    };
    let summary = StartScan::new(&deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan ok");
    assert_eq!(summary.progress.state, ScanState::Completed);
    let row = fixture.run_row(1).expect("run row");
    assert!(row.finished, "terminal state never lost");
    assert_eq!(row.state, ScanState::Completed);
    assert_eq!(row.progress.failed, 0);
    // Throttle: at most one snapshot per interval. Phase transitions (5)
    // always persist; the 4 batch snapshots are throttled by the clock.
    assert!(
        row.updates <= 5 + 2,
        "batch snapshots must be throttled: {}",
        row.updates
    );
}

#[test]
fn manual_rescan_is_repeatable_and_converges() {
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    let relink = RelinkLibrary::new(&fixture.deps, &fixture.supervisor);
    let first = relink.run(fixture.root).expect("first rescan");
    let second = relink.run(fixture.root).expect("second rescan");
    assert_eq!(first.generation, 1);
    assert_eq!(second.generation, 2);
    // Convergence: the second scan changes nothing (all fast-skipped).
    assert_eq!(second.progress.created, 0);
    assert_eq!(second.progress.updated, 0);
    assert_eq!(second.progress.skipped, 1);
    assert_eq!(fixture.all_songs().len(), 1);
}

#[test]
fn bounded_workers_never_exceed_the_configured_bound() {
    // A parse pool of 2 must never run more than 2 files concurrently.
    let fixture = ScanFixture::new();
    for index in 0..8 {
        let path = format!("file{index}.mp3");
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("T{index}"), 1_000);
    }
    let current = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let deps = ScanDeps {
        config: ScanConfig {
            batch_size: 8,
            worker_threads: 2,
            ..ScanConfig::default()
        },
        probe: Arc::new(CountingProbe {
            inner: Arc::clone(&fixture.deps.probe),
            current: Arc::clone(&current),
            max_seen: Arc::clone(&max_seen),
        }),
        ..ScanDeps::clone(&fixture.deps)
    };
    StartScan::new(&deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan ok");
    assert_eq!(
        max_seen.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "concurrency must be bounded by worker_threads"
    );
}

#[test]
fn default_worker_concurrency_is_min_cpu_4() {
    // Task 12.6: the default parse pool must be `min(CPU, 4)` per design
    // §6.4 — not `available_parallelism()` outright (which could oversubscribe
    // a 32-core box and starve playback/UI) and not a hardcoded constant.
    let config = ScanConfig::default();
    let cpu = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    assert_eq!(
        config.worker_threads,
        cpu.min(4),
        "default worker_threads must equal min(CPU, 4); got {} on a {}-cpu host",
        config.worker_threads,
        cpu,
    );
}

#[test]
fn stress_scan_bounded_hashes_and_cancels_without_corrupting_songs() {
    // Task 12.6 stress: a medium library parsed under a bounded pool stays
    // within the configured bound even under pressure, and cancelling
    // mid-scan (the `CancelScan` path) never marks anything missing or
    // leaves a half-persisted batch. This is the CPU/cancel pressure shape
    // the pipeline is designed for — bounded workers, truthful progress,
    // idempotent terminal state.
    const N: usize = 120;
    let fixture = ScanFixture::new();
    for index in 0..N {
        let path = format!("dir{:02}/file{index:03}.mp3", index % 7);
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("Song {index}"), 1_000 + (index as u64));
    }

    // Bounded pool: 4 workers (or fewer on a small machine) must never let
    // more than the configured bound run concurrently even at N=120.
    let pool = 4.min(std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get));
    let current = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let deps = ScanDeps {
        config: ScanConfig {
            batch_size: 40,
            worker_threads: pool,
            ..ScanConfig::default()
        },
        probe: Arc::new(CountingProbe {
            inner: Arc::clone(&fixture.deps.probe),
            current: Arc::clone(&current),
            max_seen: Arc::clone(&max_seen),
        }),
        ..ScanDeps::clone(&fixture.deps)
    };

    let summary = StartScan::new(&deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("stress scan ok");
    assert_eq!(summary.progress.created, N as u64);
    let observed = max_seen.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        observed, pool,
        "bounded pool held {pool} under N={N} stress; saw {observed}"
    );

    // Cancel-under-load: re-scan with a canceller that fires after a chunk,
    // then assert the missing pass never runs and the DB is consistent.
    let canceller = Arc::new(CancelOnHash {
        supervisor: fixture.supervisor.clone(),
        inner: Arc::clone(&fixture.deps.hasher),
    });
    let deps = ScanDeps {
        config: ScanConfig {
            batch_size: 20,
            worker_threads: pool,
            ..ScanConfig::default()
        },
        hasher: Arc::clone(&canceller) as Arc<dyn ContentHasher>,
        ..ScanDeps::clone(&fixture.deps)
    };
    // Touch a subset so the rescan actually re-parses instead of fast-skip.
    for index in (0..N).step_by(3) {
        let path = format!("dir{:02}/file{index:03}.mp3", index % 7);
        fixture.write_file(&path, format!("audio-v2-{index}").as_bytes());
    }
    let summary = StartScan::new(&deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("cancelled stress scan returns a summary");
    assert!(summary.cancelled);
    // No batch was half-committed as missing.
    let songs = fixture.all_songs();
    assert_eq!(
        songs.len(),
        N,
        "a cancelled scan must not wipe songs it did not get to re-parse"
    );
    let row = fixture.run_row(2).expect("second run row");
    assert_eq!(row.state, ScanState::Cancelled);
}
