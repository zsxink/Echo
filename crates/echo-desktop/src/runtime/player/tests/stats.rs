use super::*;

#[test]
fn library_durability_recorder_never_counts_before_directory_commit() {
    use echo_core::application::ports::LibraryRepository;
    use echo_core::domain::entities::{LibraryRoot, Song};
    use echo_core::domain::library::{LibraryId, LibraryManifest};
    use echo_core::infrastructure::filesystem::RootControlPlane;
    let directory = tempfile::tempdir().unwrap();
    let routed = crate::runtime::app::assemble(
        &directory.path().join("local.db"),
        &directory.path().join("covers"),
    )
    .unwrap();
    let root = LibraryRootId::new();
    let path = directory.path().join("library");
    std::fs::create_dir(&path).unwrap();
    routed.registry.register(root, &path);
    LibraryRepository::upsert(
        routed.database.as_ref(),
        &LibraryRoot::new(root, path, true, true),
    )
    .unwrap();
    routed
        .deps
        .control
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "recorder"),
        )
        .unwrap();
    let song = Song::new(
        SongId::new(),
        root,
        RelativeMediaPath::new("media/test.flac").unwrap(),
        Revision::from_u64(1),
    );
    SongRepository::upsert(routed.database.as_ref(), &song).unwrap();
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_reached = reached.clone();
    let control = Arc::new(
        RootControlPlane::new(routed.registry.clone()).with_commit_hook(move |at, _| {
            if at == "prepared" {
                hook_reached.store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(echo_core::error::Error::unavailable(
                    "recorder directory",
                    "injected failure",
                ));
            }
            Ok(())
        }),
    );
    routed.database.configure_library_control(control.clone());
    let recorder = super::super::recorder::CorePlaybackRecorder::new(
        routed.database.clone(),
        control,
        routed.deps.device_id.clone(),
        routed.deps.roots.clone(),
        routed.deps.activity.clone(),
    );
    assert!(recorder
        .record(PlaybackSessionId::new(), song.id())
        .is_err());
    assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        SongRepository::by_id(routed.database.as_ref(), song.id())
            .unwrap()
            .unwrap()
            .play_count()
            .as_u64(),
        0
    );
    assert!(routed
        .database
        .device_play_counts(song.id())
        .unwrap()
        .is_empty());
}

#[test]
fn stats_recorder_records_a_qualified_library_listen_exactly_once() {
    // Regression: PlaybackStatsRecorder had no production caller, so
    // play_count never moved and 最近播放 stayed empty. The watcher must
    // feed the real snapshot stream and fire the one RecordPlayback once
    // the real monotonic clock crosses the threshold (no force_accumulate).
    let spy = Arc::new(SpySink::default());
    let sink: Arc<dyn PlaybackRecorder> = spy.clone();
    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake.clone();
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    spawn_stats_recorder(port.clone(), coordinator.clone(), sink);

    let song = SongId::new();
    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![song],
            selected_index: 0,
        });
    }
    // duration 0.2s → threshold min(30s, 50%) = 0.1s of *real* listening.
    fake.set_duration(0.2);
    fake.set_state(PlaybackState::Playing);
    std::thread::sleep(std::time::Duration::from_millis(300));
    fake.set_state(PlaybackState::Paused); // settle point fires the record

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let calls = spy.0.lock().unwrap().clone();
        if calls.len() == 1 {
            assert_eq!(calls[0].1, song);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let calls = spy.0.lock().unwrap().clone();
    panic!("expected exactly one record call, got {calls:?}");
}

#[test]
fn stats_recorder_never_records_temporary_items() {
    // 临时播放项不计统计: a temporary load must not reach the sink even
    // after listening well past the threshold.
    let spy = Arc::new(SpySink::default());
    let sink: Arc<dyn PlaybackRecorder> = spy.clone();
    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake.clone();
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    spawn_stats_recorder(port.clone(), coordinator.clone(), sink);

    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_temporary(crate::player::coordinator::TemporaryPlay {
            display_name: "temp".into(),
            path: std::path::PathBuf::from("/tmp/echo-test.mp3"),
            duration: Some(0.2),
            metadata: crate::player::queue::TemporaryMetadata::default(),
            on_active_root: false,
        });
    }
    fake.set_state(PlaybackState::Playing);
    std::thread::sleep(std::time::Duration::from_millis(300));
    fake.set_state(PlaybackState::Paused);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        spy.0.lock().unwrap().is_empty(),
        "temporary playback must never be recorded"
    );
}
