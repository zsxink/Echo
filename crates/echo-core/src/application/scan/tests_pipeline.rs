//! Pipeline behaviour: external-missing/pending-delete treatment, batched reconcile, diagnostics and cancellation.

#[allow(clippy::wildcard_imports)]
use super::*;

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
