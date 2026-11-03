use super::*;

/// A frame provider over a coordinator: one lock per frame, and the queue
/// projection is built only when the revision changed since `since`.
///
/// This mirrors the shell wiring — the change criterion lives with the
/// provider, so the forwarder itself stays free of queue knowledge.
fn frames(
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
) -> PlayerFrameProvider {
    Arc::new(move |since| {
        let coord = coordinator.lock().expect("coordinator lock");
        let revision = coord.queue_revision();
        let projection = if since == Some(revision) {
            None
        } else {
            Some(QueueProjection {
                revision,
                entries: coord.queue_view(),
                failed_round: coord.failed_round().collect(),
                blocked: coord.blocked_entries().into_iter().collect(),
                current: coord.current().map(|entry| entry.id),
            })
        };
        PlayerFrame {
            view: CoordinatorView {
                current: coord.current().cloned(),
                mode: coord.mode(),
                revision,
            },
            projection,
        }
    })
}

/// An effects provider that never changes: the effects stream is not what
/// these cases assert.
///
/// The snapshot is built **once**: rendering an effects snapshot runs the
/// frequency sweep, and doing that per frame would make these cases measure
/// the analysis instead of the queue projection.
fn idle_effects() -> EffectsSnapshotProvider {
    let snapshot = EffectsSnapshotDto::from(crate::effects::EffectsState::default().snapshot());
    Arc::new(move || snapshot.clone())
}

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
    let (queue_tx, queue_rx) = std::sync::mpsc::channel();
    // A metadata resolver over the empty in-memory database: the queue has
    // no library songs to resolve here, but the forwarder must ask for
    // metadata with every snapshot (task 2.2) and never fail on an empty
    // result.
    let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
    let metadata = Arc::new(QueueMetadataResolver::new(db.clone(), db));
    spawn_forwarder(
        controller.port.clone(),
        frames(controller.coordinator.clone()),
        metadata,
        idle_effects(),
        PlayerEmitters {
            snapshot: Box::new(move |ui| {
                let _ = tx.send(ui);
            }),
            queue: Box::new(move |ui| {
                let _ = queue_tx.send(ui);
            }),
            effects: Box::new(|_| {}),
        },
    );

    {
        let mut coord = controller.coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![song],
            selected_index: 0,
        });
    }

    let ui = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the forwarder must emit a UI snapshot after a play command");
    assert_eq!(ui.state, "playing");
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert!(
        ui.current_queue_entry_id.is_some(),
        "the player bar keys its non-empty 当前播放区 off this id"
    );
    let queue = queue_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the forwarder must emit the queue projection");
    assert_eq!(queue.entries.len(), 1);
    assert!(queue.entries[0].is_current);
}

#[test]
fn snapshot_forwarder_emits_queue_projection_only_when_revision_changes() {
    // The whole point of the split: the position stream runs at 10 Hz, but the
    // queue projection is rebuilt and published only when the queue actually
    // changed. Without this the 480-entry projection was rebuilt and shipped
    // ten times a second (issue #112).
    let controller = PlayerController::over_fake(FakePlayer::new());
    let song = SongId::new();
    let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
    let (queue_tx, queue_rx) = std::sync::mpsc::channel();
    let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
    let metadata = Arc::new(QueueMetadataResolver::new(db.clone(), db));
    spawn_forwarder(
        controller.port.clone(),
        frames(controller.coordinator.clone()),
        metadata,
        idle_effects(),
        PlayerEmitters {
            snapshot: Box::new(move |ui| {
                let _ = snapshot_tx.send(ui);
            }),
            queue: Box::new(move |ui| {
                let _ = queue_tx.send(ui);
            }),
            effects: Box::new(|_| {}),
        },
    );

    {
        let mut coord = controller.coordinator.lock().expect("coordinator lock");
        coord.play_context(&ViewContext {
            songs: vec![song],
            selected_index: 0,
        });
    }

    // The queue changed → exactly one projection.
    let first = queue_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the changed queue is published");
    assert_eq!(first.entries.len(), 1);
    assert!(
        queue_rx.try_recv().is_err(),
        "one queue change publishes exactly one projection"
    );

    // Transport-only frames do not change the queue → no projection.
    for _ in 0..5 {
        controller
            .port
            .send(PlayerCommand::SetVolume(0.5))
            .expect("volume");
    }
    let mut snapshots = 0usize;
    while snapshots < 5 {
        snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the forwarder keeps emitting position frames");
        snapshots += 1;
    }
    assert!(
        queue_rx.try_recv().is_err(),
        "an unchanged queue is not published again"
    );

    // A queue change publishes the projection again, with a newer revision.
    // The forwarder is driven by the actor's snapshot stream, so the change
    // becomes visible on the next transport frame — the same latency the
    // single-stream design had.
    {
        let mut coord = controller.coordinator.lock().expect("coordinator lock");
        coord.enqueue(QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(SongId::new()),
        });
    }
    controller
        .port
        .send(PlayerCommand::SetVolume(0.4))
        .expect("volume");
    let second = queue_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the changed queue is published again");
    assert!(
        second.revision > first.revision,
        "the projection carries a strictly newer revision"
    );
    assert_eq!(second.entries.len(), 2);
}

#[test]
fn effects_are_emitted_only_when_the_snapshot_sequence_advances() {
    // The effects snapshot travels on its own stream. It must not repeat for
    // every position frame: it travels when its content changed, which the
    // sequence reports (AE-R07).
    let controller = PlayerController::over_fake(FakePlayer::new());
    let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
    let (effects_tx, effects_rx) = std::sync::mpsc::channel();
    let db = Arc::new(echo_core::application::testing::memory_database::MemoryDatabase::new());
    let metadata = Arc::new(QueueMetadataResolver::new(db.clone(), db));
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let reported = sequence.clone();
    let effects: EffectsSnapshotProvider = Arc::new(move || {
        let mut snapshot =
            EffectsSnapshotDto::from(crate::effects::EffectsState::default().snapshot());
        snapshot.snapshot_sequence = Some(reported.load(std::sync::atomic::Ordering::SeqCst));
        snapshot
    });
    spawn_forwarder(
        controller.port.clone(),
        frames(controller.coordinator.clone()),
        metadata,
        effects,
        PlayerEmitters {
            snapshot: Box::new(move |ui| {
                let _ = snapshot_tx.send(ui);
            }),
            queue: Box::new(|_| {}),
            effects: Box::new(move |dto| {
                let _ = effects_tx.send(dto);
            }),
        },
    );

    // The first frame publishes the effects snapshot.
    controller
        .port
        .send(PlayerCommand::SetVolume(0.5))
        .expect("volume");
    let first = effects_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the effects snapshot is published");
    assert_eq!(first.snapshot_sequence, Some(1));

    // Further position frames do not repeat it.
    for _ in 0..5 {
        controller
            .port
            .send(PlayerCommand::SetVolume(0.4))
            .expect("volume");
    }
    for _ in 0..5 {
        snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the forwarder keeps emitting position frames");
    }
    assert!(
        effects_rx.try_recv().is_err(),
        "an unchanged effects snapshot is not published again"
    );

    // A changed effects snapshot is published with a newer sequence.
    sequence.store(2, std::sync::atomic::Ordering::SeqCst);
    controller
        .port
        .send(PlayerCommand::SetVolume(0.3))
        .expect("volume");
    let second = effects_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the changed effects snapshot is published");
    assert_eq!(second.snapshot_sequence, Some(2));
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
