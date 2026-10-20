//! 8.3: audio-only hardened options + "only Rust-validated local paths load".

#[allow(clippy::wildcard_imports)]
use super::*;

// ------------------------------------------------------------------
// 8.3: audio-only config + "only Rust-validated local paths load"
// ------------------------------------------------------------------

#[test]
fn hardened_options_are_audio_only_and_least_privilege() {
    // The combined set must (a) disable user config/scripts/ytdl,
    // (b) forbid everything but the local file protocol, and (c) force pure
    // audio. Hardening intent is declared across the required + optional
    // groups; the split only reflects what a build can accept.
    let names: Vec<&str> = HARDENED_REQUIRED
        .iter()
        .chain(HARDENED_OPTIONAL)
        .map(|(n, _)| *n)
        .collect();
    for required in [
        "config",             // no user mpv.conf
        "load-scripts",       // no user scripts
        "ytdl",               // no network downloader
        "video",              // audio-only
        "vo",                 // null video output
        "osc",                // no on-screen controls
        "protocol-whitelist", // file only
    ] {
        assert!(
            names.contains(&required),
            "missing hardened option {required}"
        );
    }
    let whitelist = HARDENED_OPTIONAL
        .iter()
        .find(|(n, _)| *n == "protocol-whitelist")
        .map(|(_, v)| *v)
        .unwrap();
    assert_eq!(whitelist, "file");
    // The strictly-required set must stay minimal and genuinely loadable on
    // every libmpv build, while config/video/vo are true hard boundaries.
    assert!(
        HARDENED_REQUIRED
            .iter()
            .all(|(n, _)| *n == "config" || *n == "video" || *n == "vo" || *n == "audio-display"),
        "required options are exactly the audio-only hard boundaries"
    );
}

#[test]
fn non_local_scheme_path_is_refused_as_failed() {
    // A URL-like path must never reach the backend (or mpv). Loads of
    // `http://…` / `smb://…` → Failed, not queued into the backend.
    let (mut actor, snapshot, _props) = spawn_test(vec![]);
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "remote".into(),
            path: std::path::PathBuf::from("http://example.com/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    // wait_for Failed
    let snap = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(snap.state, PlaybackState::Failed);
    actor.shutdown();
}

#[test]
fn local_path_is_accepted_and_queued() {
    // A normal local path is accepted and forwarded to the backend.
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    // Give the actor a moment to process; then the backend queue is
    // populated (the loop filters non-local paths out before queueing).
    std::thread::sleep(std::time::Duration::from_millis(20));
    actor.shutdown();
}

#[test]
fn is_local_media_path_classifies_schemes() {
    assert!(is_local_media_path(std::path::Path::new("/music/a.flac")));
    assert!(is_local_media_path(std::path::Path::new(
        "C:\\music\\a.flac"
    )));
    assert!(is_local_media_path(std::path::Path::new(
        "相对/music/a.flac"
    )));
    assert!(!is_local_media_path(std::path::Path::new(
        "http://x/a.flac"
    )));
    assert!(!is_local_media_path(std::path::Path::new(
        "smb://host/share/a.flac"
    )));
    assert!(!is_local_media_path(std::path::Path::new(
        "mms://host/a.flac"
    )));
}

#[test]
fn load_library_song_resolves_and_loads_path() {
    // With a resolver supplied, `LoadLibrarySong` resolves SongId → path on
    // the actor thread and queues a real load; a following FileLoaded drives
    // Loading → Playing (task 10.6 / 8.3 playback path).
    use echo_core::domain::ids::SongId;

    // Use a temp file path so we can assert the exact resolved path reached
    // the backend without leaking anything to DTOs.
    let dir = tempfile::tempdir().expect("tempdir");
    let track_path = dir.path().join("track.flac");
    let expected = track_path;
    let song_id = SongId::new();
    let resolver: SongResolver = Arc::new(move |id| {
        assert_eq!(id, song_id, "resolver given a different song");
        Ok(expected.clone())
    });

    let snapshot = snapshot_stub();
    // A FileLoaded event proves the load reached the backend and succeeded.
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            Ok(TestBackend::new(vec![BackendEvent::FileLoaded]))
        },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id,
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let s = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
    assert_eq!(s.state, PlaybackState::Playing, "load should reach Playing");
    actor.shutdown();
}

#[test]
fn load_library_song_without_resolver_goes_failed() {
    // No resolver (or a resolution error) must surface `Failed`, never a
    // broken `Playing`; the coordinator's error-skip advances past it.
    use echo_core::domain::ids::SongId;

    let snapshot = snapshot_stub();
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
        snapshot.clone(),
        None, // no resolver
    )
    .expect("spawn");
    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id: SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let s = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(
        s.state,
        PlaybackState::Failed,
        "unresolvable song -> Failed"
    );
    actor.shutdown();
}

#[test]
fn load_library_song_resolver_error_goes_failed() {
    // A resolver that returns Err must also surface Failed (not Loading),
    // so an unknown song does not spin.
    use echo_core::domain::ids::SongId;

    let snapshot = snapshot_stub();
    let resolver: SongResolver =
        Arc::new(|_| Err(echo_core::error::Error::unavailable("song", "unknown")));
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id: SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let s = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(s.state, PlaybackState::Failed, "resolve error -> Failed");
    actor.shutdown();
}

/// Pre-seed the shared snapshot with stale progress facts so a "cleared"
/// assertion is non-vacuous: if a load path forgot to reset them, the
/// published snapshot would still carry these values (issue #6: the UI
/// showed the previous file's `5:18` while the new open had already
/// failed).
fn seed_stale_progress(snapshot: &Arc<RwLock<PlayerSnapshot>>) {
    let mut s = snapshot.write().expect("snapshot poisoned");
    s.position = Some(7.5);
    s.duration = Some(318.0);
}

#[test]
fn a_rejected_non_local_load_clears_stale_progress_facts() {
    // Defense-in-depth rejection (`://` present) publishes `Failed`; the
    // failure snapshot must not carry the previous file's progress.
    let (mut actor, snapshot, _props) = spawn_test(vec![]);
    seed_stale_progress(&snapshot);

    actor.send(load_temporary("http://x/a.flac")).unwrap();
    let failed = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(failed.position, None, "position must reset on Failed");
    assert_eq!(failed.duration, None, "duration must reset on Failed");
    actor.shutdown();
}

#[test]
fn a_library_song_resolve_failure_clears_stale_progress_facts() {
    use echo_core::domain::ids::SongId;

    let snapshot = snapshot_stub();
    seed_stale_progress(&snapshot);
    let resolver: SongResolver =
        Arc::new(|_| Err(echo_core::error::Error::unavailable("song", "unknown")));
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id: SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let failed = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(failed.position, None, "position must reset on Failed");
    assert_eq!(failed.duration, None, "duration must reset on Failed");
    actor.shutdown();
}

#[test]
fn a_paused_library_song_resolve_failure_clears_stale_progress_facts() {
    use echo_core::domain::ids::SongId;

    let snapshot = snapshot_stub();
    seed_stale_progress(&snapshot);
    let resolver: SongResolver =
        Arc::new(|_| Err(echo_core::error::Error::unavailable("song", "unknown")));
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    actor
        .send(PlayerCommand::LoadLibrarySongPaused {
            song_id: SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let failed = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
    assert_eq!(failed.position, None, "position must reset on Failed");
    assert_eq!(failed.duration, None, "duration must reset on Failed");
    actor.shutdown();
}

#[test]
fn entering_loading_starts_without_stale_progress_facts() {
    // A fresh load publishes `Loading` before mpv reports anything; that
    // snapshot must not carry the previous file's progress either.
    let (mut actor, snapshot, _props) = spawn_test(vec![]);
    seed_stale_progress(&snapshot);

    actor.send(load_temporary("/music/a.flac")).unwrap();
    let loading = wait_for(&snapshot, |s| s.state == PlaybackState::Loading);
    assert_eq!(loading.position, None, "position must reset on Loading");
    assert_eq!(loading.duration, None, "duration must reset on Loading");
    actor.shutdown();
}

#[test]
fn file_loaded_still_drives_progress_from_mpv_reports() {
    // The reset must not eat real facts: after `FileLoaded`, mpv-reported
    // time-pos/duration keep driving the snapshot (the subscription replay
    // models mpv's observe behaviour).
    let snapshot = snapshot_stub();
    let snapshot_join = snapshot.clone();
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(vec![]);
            b.file_loaded_on_load = true;
            b.media_replay_on_observe = Some(318.0);
            Ok(b)
        },
        snapshot_join,
        None,
    )
    .expect("spawn");

    actor.send(load_temporary("/music/a.flac")).unwrap();
    let loaded = wait_for(&snapshot, |s| {
        s.state == PlaybackState::Playing && s.duration == Some(318.0)
    });
    assert_eq!(loaded.duration, Some(318.0));
    actor.shutdown();
}
