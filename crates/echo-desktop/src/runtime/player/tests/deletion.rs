use super::*;

#[test]
fn coordinated_delete_removes_current_song_and_commits() {
    // Regression: delete_song bypassed the DeletionCoordinator entirely,
    // so deleting the currently-playing song left the queue showing a
    // track that no longer exists, with no unload barrier (task 8.11).

    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake.clone();
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    let s1 = SongId::new();
    let s2 = SongId::new();
    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
    }
    fake.set_state(PlaybackState::Playing);

    let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = deleted.clone();
    let outcome = delete_song_coordinated(
        &coordinator,
        s1,
        move |song| {
            sink.lock().unwrap().push(song);
            Ok("op-1".into())
        },
        std::time::Duration::from_secs(2),
    )
    .expect("delete must commit");
    assert_eq!(outcome, "op-1");
    assert_eq!(deleted.lock().unwrap().as_slice(), &[s1]);

    // The queue no longer references the deleted song, and the next
    // available item aligned.
    let view = coordinator.lock().expect("coordinator lock");
    assert!(
        view.queue()
            .entries()
            .iter()
            .all(|e| e.item.song_id() != Some(s1)),
        "deleted song must leave the queue"
    );
    assert_eq!(
        view.current().and_then(|e| e.item.song_id()),
        Some(s2),
        "next entry becomes current"
    );
    // The player was stopped through the port (unload barrier path ran).
    assert_eq!(fake.snapshot().state, PlaybackState::Stopped);
}

#[test]
fn coordinated_delete_rolls_back_when_core_refuses() {
    let fake = Arc::new(FakePlayer::new());
    let port: Arc<dyn PlayerPort> = fake;
    let coordinator = Arc::new(Mutex::new(PlaybackCoordinator::new(port.clone())));
    let s1 = SongId::new();
    let s2 = SongId::new();
    {
        let mut coord = coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
    }

    let outcome = delete_song_coordinated(
        &coordinator,
        s1,
        |_song| Err(echo_core::error::Error::unavailable("song", "locked")),
        std::time::Duration::from_secs(2),
    );
    assert!(
        outcome.is_err(),
        "a Core refusal must roll back, not fake success"
    );

    // The queue was restored: the target is still current.
    let view = coordinator.lock().expect("coordinator lock");
    assert_eq!(
        view.current().and_then(|e| e.item.song_id()),
        Some(s1),
        "rollback keeps the current entry"
    );
    assert_eq!(view.queue().entries().len(), 2);
}
