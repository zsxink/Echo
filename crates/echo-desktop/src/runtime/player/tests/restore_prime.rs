use super::*;

#[test]
fn restore_or_prime_restores_a_persisted_session_paused() {
    // 冷启动恢复: a persisted session is rebuilt paused — never a sound.
    let controller = PlayerController::over_fake(FakePlayer::new());
    let s1 = SongId::new();
    let s2 = SongId::new();
    let session = crate::player::session::snapshot_queue(
        &ViewContext {
            songs: vec![s1, s2],
            selected_index: 1,
        }
        .build_queue(),
        PlayMode::Shuffle,
        0.5,
        false,
        Some(3.0),
        Some("playlist:p9"),
    );
    let store = MemSession::with(session);
    let outcome = restore_or_prime_playback(
        &controller.coordinator,
        &store,
        |_| vec![],
        || panic!("default view must not be queried when a session restores"),
    );
    assert_eq!(outcome, "restored");
    let coord = controller.coordinator.lock().expect("lock");
    assert_eq!(coord.snapshot().state, PlaybackState::Paused);
    assert!(
        (coord.snapshot().volume - 0.5).abs() < 1e-9,
        "volume is restored from the session"
    );
    assert_eq!(coord.mode(), PlayMode::Shuffle);
    drop(coord);
}

#[test]
fn restore_or_prime_primes_the_first_song_of_the_default_view() {
    // Nothing persisted + a non-empty library: the newest candidate is
    // primed into the player bar paused, in the default mode.
    let controller = PlayerController::over_fake(FakePlayer::new());
    let store = MemSession::empty();
    let s1 = SongId::new();
    let s2 = SongId::new();
    let outcome =
        restore_or_prime_playback(&controller.coordinator, &store, |_| vec![], || vec![s1, s2]);
    assert_eq!(outcome, "primed");
    let coord = controller.coordinator.lock().expect("lock");
    assert_eq!(coord.snapshot().state, PlaybackState::Paused);
    assert_eq!(coord.mode(), PlayMode::Sequential, "default is 列表循环");
    assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
    assert_eq!(coord.queue().len(), 2, "the supplied context stays intact");
    drop(coord);
}

#[test]
fn restore_or_prime_replaces_a_dropped_current_with_the_newest_candidate() {
    let controller = PlayerController::over_fake(FakePlayer::new());
    let old = SongId::new();
    let newest = SongId::new();
    let mut session = crate::player::session::snapshot_queue(
        &ViewContext {
            songs: vec![old],
            selected_index: 0,
        }
        .build_queue(),
        PlayMode::Shuffle,
        0.42,
        true,
        Some(12.0),
        None,
    );
    session.current = Some(format!("{}:{}", QueueEntryId::new(), SongId::new()));

    let outcome = restore_or_prime_playback(
        &controller.coordinator,
        &MemSession::with(session),
        |_| vec![],
        || vec![newest],
    );

    assert_eq!(outcome, "primed");
    let coord = controller.coordinator.lock().expect("lock");
    assert_eq!(coord.current().unwrap().item.song_id(), Some(newest));
    assert_eq!(coord.queue().len(), 1);
    assert_eq!(coord.mode(), PlayMode::Shuffle);
    assert_eq!(coord.snapshot().state, PlaybackState::Paused);
    assert!((coord.snapshot().volume - 0.42).abs() < f64::EPSILON);
    assert!(coord.snapshot().muted);
    drop(coord);
}

#[test]
fn restore_or_prime_yields_an_empty_bar_for_an_empty_library() {
    let controller = PlayerController::over_fake(FakePlayer::new());
    let store = MemSession::empty();
    let outcome = restore_or_prime_playback(&controller.coordinator, &store, |_| vec![], Vec::new);
    assert_eq!(outcome, "empty");
    let coord = controller.coordinator.lock().expect("lock");
    assert!(coord.queue().is_empty());
    assert_eq!(coord.snapshot().state, PlaybackState::Stopped);
    drop(coord);
}
