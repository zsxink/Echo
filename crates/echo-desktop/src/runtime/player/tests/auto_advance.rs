use super::*;

#[test]
fn auto_advance_loads_the_next_track_when_a_song_naturally_ends() {
    // Regression: a track reaching EOF published `ended` to the UI while
    // the queue stalled — `on_played_to_end` had no production caller, so
    // 唱完一首永远不会自动接下一首. The watcher must turn the Playing →
    // Ended transition into a queue advance through the real snapshot
    // subscription path.
    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake.clone();
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    spawn_auto_advance(port.clone(), coordinator.clone());

    let s1 = SongId::new();
    let s2 = SongId::new();
    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
    }
    assert_eq!(fake.last_loaded_song(), Some(s1));

    // The track plays out; the actor transitions Playing → Ended.
    fake.set_state(PlaybackState::Playing);
    fake.set_state(PlaybackState::Ended);

    // The watcher runs on its own thread; poll for the advance.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if fake.last_loaded_song() == Some(s2) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("auto-advance never loaded the next song after Ended");
}

#[test]
fn auto_advance_does_not_double_advance_on_a_duplicate_ended_publish() {
    // Two Ended snapshots in a row describe one end; the transition guard
    // must advance exactly once (to s2), not skip straight to a stop.
    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake.clone();
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    spawn_auto_advance(port.clone(), coordinator.clone());

    let s1 = SongId::new();
    let s2 = SongId::new();
    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
    }
    fake.set_state(PlaybackState::Ended);
    fake.set_state(PlaybackState::Ended); // duplicate publish of the same end

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if fake.last_loaded_song() == Some(s2) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("auto-advance never loaded the next song after Ended");
}
