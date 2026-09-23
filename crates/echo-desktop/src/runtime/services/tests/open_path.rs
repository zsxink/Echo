use super::*;

// ==== system file-open dispatch (SFI-R06, normalize-os-file-open-paths) ====
//
// The shell hands a decoded absolute path to `play_temporary_file`; its first
// move is `active_song_for_path`, whose answer decides between "play the
// existing library identity" and "create a session temporary item". These
// tests pin the predicate the dispatch depends on — the coordinator's
// temporary-item behavior itself is covered by
// `coordinator::tests::play_temporary_is_session_only`.

#[test]
fn open_path_outside_the_active_library_is_not_a_library_uuid() {
    // SFI-R06-S01: a system-opened file that matches no library record must
    // resolve to `None`, so the caller creates a temporary item instead of
    // borrowing a wrong identity.
    let f = ScanFixture::new();
    let svc = services(&f);

    let resolved = svc
        .active_song_for_path(std::path::Path::new("/tmp/We Will Rock You - Queen.flac"))
        .expect("read");

    assert_eq!(
        resolved, None,
        "a non-library file must not resolve to a song"
    );
}

#[test]
fn open_path_of_an_active_library_song_resolves_to_its_uuid() {
    // SFI-R06-S02: a path inside the active root that matches an existing
    // record resolves to that song's UUID — the caller plays the existing
    // identity (its overrides, favorites, stats) instead of a temporary item.
    let f = ScanFixture::new();
    f.write_file("a.flac", b"audio");
    f.set_audio("a.flac", "标题", 1_000);
    StartScan::new(&f.deps, &f.supervisor)
        .run(f.root)
        .expect("scan");
    let svc = services(&f);

    let root = f
        .deps
        .roots
        .active_root()
        .expect("root query")
        .expect("active root");
    let song = &f.deps.songs.all_in_root(f.root).expect("songs")[0];
    let opened = root.resolve_song_path(song).expect("resolve");

    let resolved = svc.active_song_for_path(&opened).expect("read");
    assert_eq!(
        resolved,
        Some(song.id()),
        "an in-library file must resolve to its recorded UUID"
    );
}

#[test]
fn open_path_of_a_non_active_root_never_resolves_to_the_old_uuid() {
    // SFI-R06-S03: a file under a retained-but-inactive root must not resolve
    // to the old root's song identity — the active-root isolation is not
    // bypassed by a UUID, so the file plays as a session temporary item.
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};

    let f = ScanFixture::new();
    let old_root = LibraryRootId::new();
    LibraryRepository::upsert(
        &f.database,
        &LibraryRoot::new(old_root, "/old-library".into(), false, true),
    )
    .expect("old root");
    let old_song = Song::new(
        SongId::new(),
        old_root,
        RelativeMediaPath::new("media/old.flac").unwrap(),
        Revision::INITIAL,
    );
    f.deps.songs.upsert(&old_song).expect("old song");
    let svc = services(&f);

    let resolved = svc
        .active_song_for_path(std::path::Path::new("/old-library/media/old.flac"))
        .expect("read");

    assert_eq!(
        resolved, None,
        "an old-root file must never play through the old root's UUID"
    );
}

/// D3 of deliver-file-opens-after-frontend-ready: there is exactly one startup
/// supervisor. The shell registers an `Arc<StartupSupervisor>` (so the OS
/// file-open handler can consult it before `AppServices` exists) and hands the
/// same `Arc` to the composition root — a second, independently-constructed
/// instance would let the shell's file-open delivery gate and this root's
/// readiness gate diverge, and the divergence would be invisible: both would
/// answer, just differently.
#[test]
fn the_composition_root_shares_the_shells_startup_supervisor() {
    let fixture = ScanFixture::new();
    let shell = std::sync::Arc::new(StartupSupervisor::new());
    let app = AppServices::with_runtime(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        std::sync::Arc::clone(&shell),
        std::sync::Arc::new(TestDialogs::cancelling()),
        RootRegistry::new(),
        std::sync::Arc::new(fixture.database.clone()),
        Blockers::new(),
    );

    assert!(
        std::ptr::eq(app.startup(), &*shell),
        "the composition root must expose the shell's supervisor, not a copy"
    );

    // Behavior across the two handles, not just pointer identity: a path queued
    // through the shell's handle is drained through the root's handle, and the
    // root sees the phase the shell set.
    shell.on_ready();
    assert_eq!(app.startup().phase(), crate::runtime::StartupPhase::Ready);
    assert_eq!(
        shell.receive_file_open(std::path::PathBuf::from("/music/shared.flac")),
        None,
        "queued: the frontend listener has not registered yet"
    );
    assert_eq!(
        app.startup().mark_frontend_ready(),
        vec![std::path::PathBuf::from("/music/shared.flac")]
    );
    assert_eq!(
        app.startup()
            .receive_file_open(std::path::PathBuf::from("/music/after.flac")),
        Some(std::path::PathBuf::from("/music/after.flac"))
    );
}

/// A file opened from the file browser is read for its presentation metadata
/// before it becomes a queue item. Duration is probe-owned — the tag reader
/// deliberately never fills it — and a file outside the library gets no scan
/// pass, so this read is a temporary item's only duration source. Without it
/// the queue row read 时长未知 while the progress bar, fed by the engine,
/// already showed the real length.
#[test]
fn temporary_metadata_duration_comes_from_the_probe() {
    let fixture = ScanFixture::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("西楼别序.mp3");
    let content: &[u8] = b"outside-the-library-bytes";
    std::fs::write(&path, content).unwrap();

    // The seeded tags carry only the display fields: the real tag reader
    // returns `duration: None`, so seeding one would test a situation that
    // cannot happen.
    fixture.metadata.set_bytes(
        content,
        ParsedMetadata {
            title: Some("西楼别序".to_owned()),
            artist: Some("尹昔眠".to_owned()),
            ..ParsedMetadata::default()
        },
    );
    fixture.probe.set_bytes(
        content,
        ProbeOutcome::Audio {
            format: AudioFormat::Mpeg,
            duration: Some(std::time::Duration::from_secs(227)),
        },
    );

    let metadata = services(&fixture).read_temporary_metadata(&path);

    assert_eq!(metadata.title.as_deref(), Some("西楼别序"));
    assert_eq!(metadata.artist.as_deref(), Some("尹昔眠"));
    assert_eq!(
        metadata.duration,
        Some(227.0),
        "the probe is a temporary item's only duration source"
    );
}

/// The other half of the contract: content the probe cannot classify keeps an
/// unknown duration unknown. A row showing a made-up length would be worse
/// than one admitting it does not know.
#[test]
fn temporary_metadata_keeps_an_unsupported_duration_unknown() {
    let fixture = ScanFixture::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mystery.mp3");
    let content: &[u8] = b"not-media-at-all";
    std::fs::write(&path, content).unwrap();
    fixture.probe.set_bytes(content, ProbeOutcome::Unsupported);

    let metadata = services(&fixture).read_temporary_metadata(&path);

    assert_eq!(metadata.duration, None);
}
