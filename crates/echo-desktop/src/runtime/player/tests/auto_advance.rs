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

/// Reproduces the actor's capacity-one mailbox when the subscriber has not
/// been scheduled between the last Playing snapshot and EOF. Unlike
/// `FakePlayer`'s unbounded subscriptions, the final notification is lost.
struct DroppedTerminalPort {
    terminal: PlaybackState,
    fake: Arc<FakePlayer>,
    sender: Mutex<Option<std::sync::mpsc::SyncSender<crate::player::port::PlayerSnapshot>>>,
}

impl PlayerPort for DroppedTerminalPort {
    fn send(&self, command: PlayerCommand) -> Result<(), crate::player::port::PlayerError> {
        self.fake.send(command)
    }

    fn snapshot(&self) -> crate::player::port::PlayerSnapshot {
        self.fake.snapshot()
    }

    fn subscribe_snapshots(
        &self,
    ) -> std::sync::mpsc::Receiver<crate::player::port::PlayerSnapshot> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.try_send(self.fake.snapshot()).unwrap();
        self.fake.set_state(self.terminal);
        assert!(matches!(
            tx.try_send(self.fake.snapshot()),
            Err(std::sync::mpsc::TrySendError::Full(_))
        ));
        *self.sender.lock().unwrap() = Some(tx);
        rx
    }
}

#[test]
fn cold_restore_advances_even_when_the_final_notification_is_dropped() {
    for (count, terminal) in [
        (1, PlaybackState::Ended),
        (2, PlaybackState::Ended),
        (2, PlaybackState::Failed),
    ] {
        let fake = Arc::new(FakePlayer::new());
        let lossy = Arc::new(DroppedTerminalPort {
            terminal,
            fake: fake.clone(),
            sender: Mutex::new(None),
        });
        let port: Arc<dyn PlayerPort> = lossy.clone();
        let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
        let songs: Vec<_> = (0..count).map(|_| SongId::new()).collect();
        let persisted = crate::player::session::snapshot_queue(
            &ViewContext {
                songs: songs.clone(),
                selected_index: 0,
            }
            .build_queue(),
            PlayMode::Sequential,
            1.0,
            false,
            Some(0.0),
            None,
        );
        assert_eq!(
            restore_or_prime_playback(
                &coordinator,
                &MemSession::with(persisted),
                |_| vec![],
                Vec::new,
            ),
            "restored"
        );
        port.send(PlayerCommand::Play).unwrap();
        let restored_session = fake.last_session();
        spawn_auto_advance(port, coordinator.clone());
        assert!(
            wait_until(
                || fake.last_session() != restored_session,
                std::time::Duration::from_secs(2),
            ),
            "{count}-song restored queue stalled after a dropped {terminal:?} notification"
        );
        assert_eq!(fake.last_loaded_song(), Some(songs[count - 1]));
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);
        // The authoritative Ended state is handled only once, including when
        // a one-song list loops onto the same queue entry.
        let next_session = fake.last_session();
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert_eq!(fake.last_session(), next_session);
        lossy.sender.lock().unwrap().take();
    }
}
