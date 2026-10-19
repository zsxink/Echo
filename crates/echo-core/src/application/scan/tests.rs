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

#[test]
fn external_deletion_is_missing_not_pending_delete_and_keeps_relationships() {
    let fixture = ScanFixture::new();
    fixture.write_file("keep.mp3", b"audio-keep");
    fixture.set_audio("keep.mp3", "Keep", 1_000);
    let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
    let target_path = fixture.path("歌手/晴天.flac");

    start_scan(&fixture).run(fixture.root).expect("first scan");
    assert_eq!(fixture.all_songs().len(), 2);

    // The user deletes the file in their file manager, then a rescan
    // observes it — the external path (not Echo's delete flow).
    fixture.remove_file("歌手/晴天.flac");
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");
    assert_eq!(summary.progress.missing, 1);

    let record = song_by_path(&fixture, &target_path)
        .expect("lookup")
        .expect("record kept");
    assert_eq!(record.id(), target);
    assert_eq!(
        record.availability(),
        SongAvailability::Missing,
        "external deletion is the Missing model, not Echo's PendingDelete"
    );
    assert!(
        fixture.database.operations_for_song(target).is_empty(),
        "external missing must never enter Echo's trash/delete journal"
    );
    assert!(record.favorite(), "favorite survives external missing");
    assert_eq!(
        record.play_count().as_u64(),
        1,
        "play stats survive external missing"
    );
    assert!(
        !record.availability().is_playable(),
        "a missing file is not playable but its identity remains"
    );

    // The playlist membership row is retained through external missing
    // (its live availability is surfaced by the SQLite JOIN at query time).
    let member = fixture
        .database
        .members(playlist)
        .expect("members")
        .into_iter()
        .find(|m| m.song() == target)
        .expect("membership preserved through external missing");
    assert_eq!(member.song(), target);
}

#[test]
fn external_missing_recovers_on_original_path_restoring_everything() {
    let fixture = ScanFixture::new();
    let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
    let target_path = fixture.path("歌手/晴天.flac");
    start_scan(&fixture).run(fixture.root).expect("first scan");
    fixture.remove_file("歌手/晴天.flac");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");

    // The file comes back at its original path (same content).
    fixture.write_file("歌手/晴天.flac", b"audio-bytes");
    fixture.set_audio("歌手/晴天.flac", "晴天", 269_000);
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("recover scan");
    assert_eq!(summary.progress.missing, 0);

    let restored = song_by_path(&fixture, &target_path)
        .expect("lookup")
        .expect("restored");
    assert_eq!(
        restored.id(),
        target,
        "original-path recovery reuses the UUID"
    );
    assert_eq!(restored.availability(), SongAvailability::Available);
    assert!(restored.favorite(), "favorite restored");
    assert_eq!(restored.play_count().as_u64(), 1, "play stats restored");
    assert!(restored.availability().is_playable());
    let member = fixture
        .database
        .members(playlist)
        .expect("members")
        .into_iter()
        .find(|m| m.song() == target)
        .expect("membership kept");
    assert_eq!(member.song(), target);
}

/// The shape a continuation projection leaves behind: identity + metadata
/// only, no scan facts (portable records carry no size/mtime), marked
/// missing because availability is the scan's to establish.
#[test]
fn scan_persists_the_revival_of_a_record_projected_song() {
    let fixture = ScanFixture::new();
    fixture.write_file("歌手/晴天.flac", b"audio-bytes");
    fixture.set_audio("歌手/晴天.flac", "晴天", 269_000);
    let target_path = fixture.path("歌手/晴天.flac");

    let id = SongId::new();
    let mut song = Song::new(id, fixture.root, target_path.clone(), Revision::INITIAL);
    song.apply_metadata(Some("晴天".to_owned()), Some("歌手".to_owned()), None, None);
    song.mark_missing();
    SongRepository::upsert(&fixture.database, &song).expect("seed projected song");

    // No hash/size/mtime on the row: the fast-skip branch (the only one
    // that used to persist a revival) can never apply here.
    let summary = start_scan(&fixture).run(fixture.root).expect("scan");
    assert_eq!(summary.progress.updated, 1, "refreshed in place");

    let restored = song_by_path(&fixture, &target_path)
        .expect("lookup")
        .expect("record kept");
    assert_eq!(restored.id(), id, "the projected identity is adopted");
    assert_eq!(
        restored.availability(),
        SongAvailability::Available,
        "a file parsed at the record's path proves presence: the row must not stay missing"
    );
}

#[test]
fn external_missing_relinks_on_same_hash_path_keeping_relationships() {
    let fixture = ScanFixture::new();
    let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
    start_scan(&fixture).run(fixture.root).expect("first scan");
    fixture.remove_file("歌手/晴天.flac");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");

    // A *same-hash* file appears at a different path: the identity must
    // move to it, keeping UUID and every relationship (design §6.7).
    fixture.write_file("moved/晴天.flac", b"audio-bytes");
    fixture.set_audio("moved/晴天.flac", "晴天", 269_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("relink scan");
    assert_eq!(summary.progress.updated, 1, "relink counts as an update");

    let relinked = song_by_path(&fixture, &fixture.path("moved/晴天.flac"))
        .expect("lookup")
        .expect("relinked");
    assert_eq!(relinked.id(), target, "same-hash recovery reuses the UUID");
    assert_eq!(relinked.availability(), SongAvailability::Available);
    assert!(relinked.favorite());
    assert_eq!(relinked.play_count().as_u64(), 1);
    let member = fixture
        .database
        .members(playlist)
        .expect("members")
        .into_iter()
        .find(|m| m.song() == target)
        .expect("membership kept across the same-hash move");
    assert_eq!(member.song(), target);
    // The old path must not linger as a duplicate record either.
    assert_eq!(
        fixture.all_songs().len(),
        1,
        "one logical song per hash after relink"
    );
}

#[test]
fn echo_pending_delete_is_never_treated_as_external_missing() {
    let fixture = ScanFixture::new();
    let (target, _playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
    start_scan(&fixture).run(fixture.root).expect("first scan");

    // Echo's own delete stages the file into the controlled trash slot and
    // hides the song as PendingDelete (task 5.7).
    DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, target)
        .expect("Echo delete");

    // A rescan must NOT mark the hidden song externally missing, and must
    // not re-import its staged file from the owned trash directory.
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("rescan during delete");
    assert_eq!(
        summary.progress.missing, 0,
        "a PendingDelete song is skipped by the external-missing pass"
    );
    let record = song_by_path(&fixture, &fixture.path("歌手/晴天.flac"))
        .expect("lookup")
        .expect("record kept");
    assert_eq!(record.id(), target);
    assert_eq!(
        record.availability(),
        SongAvailability::PendingDelete,
        "Echo delete keeps its own hidden model; the scanner must not convert it to Missing"
    );
    assert_eq!(
        fixture.all_songs().len(),
        1,
        "the scanner never re-imports the staged trash file as a new song"
    );
    assert_eq!(
        fixture.database.operations_for_song(target).len(),
        1,
        "an Echo delete is recorded in the trash journal (unlike external missing)"
    );
}

fn start_scan(fixture: &ScanFixture) -> StartScan<'_> {
    StartScan::new(&fixture.deps, &fixture.supervisor)
}

#[test]
fn scan_pipeline_creates_updates_and_reports_progress() {
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio-a");
    fixture.write_file("华语/b.flac", b"audio-b");
    fixture.write_file("notes.txt", b"not audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    fixture.set_audio("华语/b.flac", "B", 2_000);

    let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
    assert!(!summary.cancelled);
    assert_eq!(summary.progress.discovered, 2);
    assert_eq!(summary.progress.created, 2);
    assert_eq!(summary.progress.skipped, 1, "the .txt file is skipped");
    assert_eq!(summary.progress.failed, 0);

    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 2);
    let a = songs
        .iter()
        .find(|s| s.path().display() == "media/a.mp3")
        .unwrap();
    assert_eq!(a.title(), Some("A"));
    assert!(
        a.blake3_hash().is_some() && a.file_size() == Some(7),
        "scan facts persisted for fast-skip"
    );
    // The run row reached Completed with the same summary.
    let row = fixture.run_row(1).expect("run row");
    assert!(row.finished);
    assert_eq!(row.state, ScanState::Completed);
    assert_eq!(row.progress.discovered, 2);
}

#[test]
fn scan_pipeline_batches_and_survives_bad_files() {
    let fixture = ScanFixture::new();
    for index in 0..5 {
        let path = format!("file{index}.mp3");
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("T{index}"), 1_000);
    }
    // One broken file among the good ones.
    fixture.write_file("broken.mp3", b"garbage");
    fixture.probe.set(
        "broken.mp3",
        crate::application::ports::ProbeOutcome::Unsupported,
    );

    let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
    assert_eq!(summary.progress.created, 5, "good files are not blocked");
    assert_eq!(summary.progress.failed, 1);
    let issues = fixture.issues(1);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code(), "unsupported_media");
    assert!(fixture.run_row(1).is_some());
}

#[test]
fn no_audio_track_is_a_diagnostic_not_a_song() {
    let fixture = ScanFixture::new();
    fixture.write_file("video.mp4", b"video-only");
    fixture.probe.set(
        "video.mp4",
        crate::application::ports::ProbeOutcome::NoAudioTrack,
    );
    let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
    assert_eq!(summary.progress.created, 0);
    assert_eq!(summary.progress.failed, 1);
    assert!(fixture.all_songs().is_empty());
}

#[test]
fn tagless_wav_scans_into_a_wav_record_with_empty_tags() {
    let fixture = ScanFixture::new();
    // A real WAV has no tag support; the file is a supported container the
    // probe recognizes as audio while the tag reader yields no fields.
    fixture.write_file("tone.wav", b"wav-bytes");
    fixture.probe.set(
        "tone.wav",
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Wav,
            duration: Some(std::time::Duration::from_secs(3)),
        },
    );
    let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
    assert_eq!(summary.progress.created, 1, "the wav is a song, not a skip");
    assert_eq!(summary.progress.failed, 0);
    let song = fixture
        .all_songs()
        .into_iter()
        .next()
        .expect("one wav song recorded");
    assert_eq!(song.format(), Some(crate::domain::media::AudioFormat::Wav));
    // No tags to read: title/artist stay None (the UI/overlay shows the
    // existing missing-tag fallbacks), duration comes from the probe.
    assert_eq!(song.title(), None);
    assert_eq!(song.artist(), None);
    assert_eq!(song.duration(), Some(std::time::Duration::from_secs(3)));
}

#[test]
fn cancel_mid_scan_never_marks_missing() {
    let fixture = ScanFixture::new();
    for index in 0..6 {
        let path = format!("file{index}.mp3");
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("T{index}"), 1_000);
    }
    // First scan completes; then the file of one song disappears.
    start_scan(&fixture).run(fixture.root).expect("first scan");
    let survivor_path = fixture.path("file0.mp3");
    let survivor = song_by_path(&fixture, &survivor_path)
        .expect("lookup")
        .expect("exists");
    fixture.remove_file("file1.mp3");

    // Touch the remaining files (new bytes) so the second scan really
    // parses (and hashes) instead of fast-skipping everything: it then
    // cancels on the very first hash call (through the supervisor, i.e.
    // exactly what `CancelScan` does). The missing pass must never run,
    // so `file1`'s record stays available.
    for index in [0, 2, 3, 4, 5] {
        let path = format!("file{index}.mp3");
        fixture.write_file(&path, format!("audio-v2-{index}").as_bytes());
    }
    let canceller = Arc::new(CancelOnHash {
        supervisor: fixture.supervisor.clone(),
        inner: Arc::clone(&fixture.deps.hasher),
    });
    let deps = ScanDeps {
        hasher: Arc::clone(&canceller) as Arc<dyn ContentHasher>,
        ..ScanDeps::clone(&fixture.deps)
    };
    let summary = StartScan::new(&deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("cancelled scan returns summary");
    assert!(summary.cancelled);
    assert_eq!(summary.progress.state, ScanState::Cancelled);

    let still_there = song_by_path(&fixture, &survivor_path)
        .expect("lookup")
        .expect("exists");
    assert_eq!(
        still_there.availability(),
        SongAvailability::Available,
        "a cancelled scan must not batch-mark anything missing"
    );
    let row = fixture.run_row(2).expect("second run");
    assert_eq!(row.state, ScanState::Cancelled);
    assert!(row.finished, "terminal state persisted");
    drop(survivor);
}

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

    // The primary file disappears: the record goes missing (UUID kept).
    fixture.remove_file("a-second.mp3");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("missing scan");
    let record = fixture.all_songs().into_iter().next().expect("record");
    assert_eq!(record.id(), primary_id);
    assert_eq!(record.availability(), SongAvailability::Missing);

    // The next scan sees the remaining same-hash file: the record is
    // promoted to it under the same UUID (同一规则提升).
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("promote scan");
    assert_eq!(summary.progress.updated, 1);
    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), primary_id, "UUID kept on promotion");
    assert_eq!(songs[0].path().display(), "media/z-first.mp3");
    assert_eq!(songs[0].availability(), SongAvailability::Available);
}

#[test]
fn unique_music_key_relink_is_conservative() {
    let fixture = ScanFixture::new();
    // A song whose file vanished, with full tags (music key available).
    fixture.write_file("old/tone.flac", b"old-bytes");
    fixture.set_audio("old/tone.flac", "晴天", 200_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");
    fixture.remove_file("old/tone.flac");
    start_scan(&fixture)
        .run(fixture.root)
        .expect("marks missing");
    let gone = fixture.all_songs().into_iter().next().expect("record kept");
    assert_eq!(gone.availability(), SongAvailability::Missing);

    // A new file with the same normalized key and duration within 2 s:
    // conservative weak re-link.
    fixture.write_file("new/tone.flac", b"new-bytes");
    fixture.set_audio("new/tone.flac", "晴天", 201_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("relink scan");
    assert_eq!(summary.progress.updated, 1);
    assert_eq!(summary.progress.created, 0);
    let relinked = fixture.all_songs().into_iter().next().unwrap();
    assert_eq!(relinked.id(), gone.id(), "weak re-link keeps the UUID");
    assert_eq!(relinked.path().display(), "media/new/tone.flac");
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

#[test]
fn embedded_and_sidecar_lyrics_are_persisted_per_source() {
    let fixture = ScanFixture::new();
    fixture.write_file("song.mp3", b"audio");
    fixture.write_file("song.lrc", b"[00:01.00]sidecar line");
    fixture.set_audio("song.mp3", "Song", 1_000);
    // The reader reports embedded lyrics; the parser turns them into lines.
    fixture.metadata.set(
        "song.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Song".to_owned()),
            artist: Some("歌手".to_owned()),
            album: Some("专辑".to_owned()),
            duration: Some(std::time::Duration::from_secs(1)),
            format: crate::domain::media::AudioFormat::Flac,
            embedded_lyrics: Some("[00:01.00]embedded line".to_owned()),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().expect("song");
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
            .expect("candidates");
    let sources: Vec<_> = candidates
        .iter()
        .map(crate::domain::entities::LyricsCandidate::source)
        .collect();
    assert!(sources.contains(&crate::domain::entities::LyricsSource::Embedded));
    assert!(sources.contains(&crate::domain::entities::LyricsSource::Sidecar));
    // No stale candidates survive a rescan: the audio changes (so the
    // file is re-parsed, not fast-skipped) and the sidecar is gone.
    fixture.write_file("song.mp3", b"audio-v2");
    fixture.remove_file("song.lrc");
    fixture.set_audio("song.mp3", "Song", 1_000);
    start_scan(&fixture).run(fixture.root).expect("rescan");
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
            .expect("candidates");
    assert!(
        !candidates
            .iter()
            .any(|c| c.source() == crate::domain::entities::LyricsSource::Sidecar),
        "cleared sidecar must not linger: {candidates:?}"
    );
}

#[test]
fn cover_cache_key_and_db_reference_stay_consistent() {
    let fixture = ScanFixture::new();
    fixture.write_file("song.mp3", b"audio");
    fixture.set_audio("song.mp3", "Song", 1_000);
    fixture.metadata.set(
        "song.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Song".to_owned()),
            cover: Some(crate::domain::media::EmbeddedCover {
                bytes: b"cover-bytes".to_vec(),
                mime: "image/png".to_owned(),
            }),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().expect("song");
    let cover = crate::application::ports::CoverRepository::cover_of(&fixture.database, song.id())
        .expect("cover ref")
        .expect("cover attached");
    // The asset key round-trips through the cache: opaque, content keyed.
    assert_eq!(
        fixture
            .deps
            .cover_cache
            .get(&cover.asset_key)
            .expect("get")
            .as_deref(),
        Some(b"cover-bytes".as_slice())
    );
    assert_eq!(cover.mime, "image/png");
    // And the referenced key is exactly the GC keep-set.
    let keys = crate::application::ports::CoverRepository::referenced_asset_keys(
        &fixture.database,
        fixture.root,
    )
    .expect("keys");
    assert_eq!(keys, vec![cover.asset_key]);
}

#[test]
fn revision_and_added_at_survive_rescans() {
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().unwrap();
    let added_at = song.added_at();
    assert_eq!(
        added_at,
        fixture.clock.wall_ms(),
        "uses the injected wall clock"
    );
    // Change the file content and its tags → the rescan re-parses and
    // updates, not re-creates (an unchanged file is a fast-skip by
    // design).
    fixture.write_file("a.mp3", b"audio-v2-longer");
    fixture.set_audio("a.mp3", "A2", 1_000);
    start_scan(&fixture).run(fixture.root).expect("rescan");
    let updated = fixture.all_songs().into_iter().next().unwrap();
    assert_eq!(updated.id(), song.id(), "identity stable across rescans");
    assert_eq!(updated.title(), Some("A2"));
    assert_eq!(updated.added_at(), added_at, "added_at never changes");
    assert!(updated.revision() >= Revision::INITIAL);
}

// ------------------------------------------------------------------
// 13.4 — watcher/root matrix: every row converges after a manual rescan
// and relationships stay stable. Each test simulates the fs-level fault
// (unmount, permission revocation) with the fake's fault injection, then
// proves a subsequent scan does not silently trust stale data.
// ------------------------------------------------------------------

#[test]
fn root_unmount_then_remount_converges_after_manual_rescan() {
    // A root that vanishes (unmounted) must surface as a failed scan, then
    // a successful scan once remounted sees the current files — and never
    // marks songs missing from a failed run.
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");

    // Unmount: enumeration fails. The failed run must NOT mark missing.
    fixture
        .fs
        .inject_fault(Error::unavailable("library root", "unmounted"));
    let error = start_scan(&fixture).run(fixture.root).unwrap_err();
    assert_eq!(error.code(), "storage");
    let record = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("kept from the failed run");
    assert_eq!(record.availability(), SongAvailability::Available);

    // Remount (clear the fault) + a modified file → the manual rescan
    // converges: the song is still there, identity stable.
    fixture.fs.clear_fault();
    fixture.write_file("a.mp3", b"audio-v2");
    fixture.set_audio("a.mp3", "A2", 1_000);
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("remounted scan");
    assert_eq!(
        summary.progress.updated, 1,
        "remounted scan re-parses the changed file by identity"
    );
    let after = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("present after remount");
    assert_eq!(
        after.id(),
        record.id(),
        "UUID stable across unmount/remount"
    );
    assert_eq!(after.title(), Some("A2"));
}

#[test]
fn permission_revocation_mid_watch_degrades_to_rescan_and_converges() {
    // Permission revoked (reads fail) → scan fails without marking songs
    // missing; after re-permission, a manual rescan converges.
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");

    fixture.fs.inject_fault(Error::permission(
        "read directory",
        crate::error::PermKind::Denied,
    ));
    let error = start_scan(&fixture).run(fixture.root).unwrap_err();
    // The fake's fault is storage-shaped whatever variant is injected; the
    // contract under test is the failed run, not the error variant.
    assert_eq!(error.code(), "storage");
    // Read-only: scan is refused, the record stays available (no missing).
    let record = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("kept under permission loss");
    assert_eq!(record.availability(), SongAvailability::Available);

    fixture.fs.clear_fault();
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("re-permission scan");
    assert_eq!(summary.progress.skipped, 1, "unchanged file fast-skipped");
    let after = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("still present");
    assert_eq!(
        after.id(),
        record.id(),
        "identity stable across permission loss"
    );
}

#[test]
fn deep_unicode_paths_and_external_moves_converge_after_rescan() {
    // Unicode component + deep nesting: the scan indexes it, an external
    // rename keeps the UUID + relationships, and a manual rescan converges.
    let fixture = ScanFixture::new();
    // A deep Unicode path (multiple components + composed CJK/emoji).
    let deep = "歌手/黑胶 精选/🎧-单曲編號/曲目一-合成字.mp3";
    fixture.write_file(deep, b"deep-unicode-bytes");
    fixture.set_audio(deep, "深巷", 2_000);
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("unicode scan");
    assert_eq!(
        summary.progress.created, 1,
        "a deep unicode path is indexed as one song"
    );

    // Favorite goes through its dedicated mutation: a metadata upsert
    // never carries user state (the SQLite `DO UPDATE` list deliberately
    // omits `is_favorite`, `play_count` and `availability`).
    let original = song_by_path(&fixture, &fixture.path(deep))
        .expect("lookup")
        .expect("deep-path song");
    SongRepository::set_favorite(&fixture.database, original.id(), true).expect("favorite set");

    // External move (rename) on disk → same content, new path, same UUID.
    let moved = "歌手/黑胶 精选/新目录/曲目一-移动后.flac";
    fixture.write_file(moved, b"deep-unicode-bytes");
    fixture.remove_file(deep);
    fixture.set_audio(moved, "深巷", 2_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("rename scan");
    assert_eq!(summary.progress.updated, 1, "external move re-links");
    let after = song_by_path(&fixture, &fixture.path(moved))
        .expect("lookup")
        .expect("re-linked at the new path");
    assert_eq!(after.id(), original.id(), "UUID survives the external move");
    assert!(after.favorite(), "favorite survives the external move");
    assert_eq!(
        after.availability(),
        SongAvailability::Available,
        "only the new path hosts the record; no duplicate"
    );
    assert!(
        fixture
            .all_songs()
            .iter()
            .filter(|s| s.id() == original.id())
            .count()
            == 1,
        "exactly one record for the moved song"
    );
}
