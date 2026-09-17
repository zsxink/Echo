use super::*;

#[test]
fn forwarder_emits_a_ui_snapshot_that_names_the_playing_song() {
    // The full pipeline the player bar depends on, headless: the coordinator
    // builds the queue and loads → the port publishes a snapshot → the
    // forwarder maps it together with the coordinator's view → the UI
    // snapshot names the current entry. Each earlier link passed its unit
    // tests while the chain as a whole was broken (a dead subscription plus
    // a queue-identity field read from the wrong source), so this asserts
    // the end of the chain rather than its parts.
    let controller = PlayerController::over_fake(FakePlayer::new());
    let song = SongId::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let coordinator = controller.coordinator.clone();
    let provider: Arc<dyn Fn() -> CoordinatorView + Send + Sync> = Arc::new(move || {
        let coord = coordinator.lock().expect("coordinator lock");
        CoordinatorView {
            entries: coord.queue_view(),
            failed_round: coord.failed_round().collect(),
            blocked: coord
                .queue_view()
                .iter()
                .filter(|entry| coord.queue().is_blocked(entry.id))
                .map(|entry| entry.id)
                .collect(),
            current: coord.current().cloned(),
            mode: coord.mode(),
        }
    });
    // A metadata resolver over the empty in-memory database: the queue has
    // no library songs to resolve here, but the forwarder must ask for
    // metadata with every snapshot (task 2.2) and never fail on an empty
    // result.
    let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
    let metadata = Arc::new(QueueMetadataResolver::new(db.clone(), db));
    spawn_forwarder(
        controller.port.clone(),
        provider,
        metadata,
        Box::new(move |ui| {
            let _ = tx.send(ui);
        }),
    );

    {
        let mut coord = controller.coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![song],
            selected_index: 0,
        });
    }

    let ui = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the forwarder must emit a UI snapshot after a play command");
    assert_eq!(ui.state, "playing");
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert!(
        ui.current_queue_entry_id.is_some(),
        "the player bar keys its non-empty 当前播放区 off this id"
    );
    assert_eq!(ui.queue.len(), 1);
    assert!(ui.queue[0].is_current);
}

#[test]
fn controller_over_fake_is_headless_and_runnable() {
    let controller = PlayerController::over_fake(FakePlayer::new());
    // The coordinator holds the port; locking and driving it must not need
    // libmpv.
    // Scoped: the coordinator lock is released as soon as the assertions are done.
    {
        let mut coord = controller.coordinator.lock().expect("lock");
        let song = SongId::new();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(song),
        };
        coord.enqueue(entry);
        // `enqueue` appends to the queue (not yet current); assert the song is
        // present so the coordinator drove a real command headlessly.
        assert!(
            coord
                .queue()
                .entries()
                .iter()
                .any(|e| e.item.song_id() == Some(song)),
            "enqueued song should appear in the queue entries"
        );
        drop(coord);
    }
}
