use super::*;

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
            metadata: Default::default(),
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
