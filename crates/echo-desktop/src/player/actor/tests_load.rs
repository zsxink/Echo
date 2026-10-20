//! Load lifecycle: command capacity, load→play state machine, stale-event supersession, paused/primed loads, effect holds interleaved with transport commands, and stale-progress cleanup on rejected loads.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn bounded_command_channel_capacity() {
    // A channel bounded at COMMAND_CAPACITY: filling it makes try_send fail
    // (non-blocking), freeing a slot lets it accept again.
    let (tx, rx) = mpsc::sync_channel::<PlayerCommand>(COMMAND_CAPACITY);
    for _ in 0..COMMAND_CAPACITY {
        tx.try_send(PlayerCommand::Play).unwrap();
    }
    assert!(tx.try_send(PlayerCommand::Play).is_err());
    assert!(rx.try_recv().is_ok());
    assert!(tx.try_send(PlayerCommand::Play).is_ok());
}

#[test]
fn load_drives_state_from_stopped_to_playing_via_file_loaded() {
    // `TestBackend::load` emits FileLoaded *after* the actor receives this
    // command, just like mpv. Pre-seeding the event races actor startup and
    // can consume it while stopped, which tests neither the load transition
    // nor the production event ordering.
    let (mut actor, snapshot, _props) = spawn_test_file_loaded_on_load();
    assert_eq!(actor.snapshot().state, PlaybackState::Stopped);

    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();

    let final_snap = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
    assert_eq!(final_snap.state, PlaybackState::Playing);
    actor.shutdown();
}

#[test]
fn subsequent_load_supersedes_and_a_stale_event_cannot_regress() {
    // Single-threaded actor: mpv serializes events, so a "late" event from
    // track A cannot arrive after B's loadfile is issued on the same loop.
    // We still verify the observable invariant: once B is loading, a
    // property event does not move the shared snapshot backward (the actor
    // only ever publishes under the current generation).
    let (mut actor, snapshot, _props) = spawn_test(vec![]);
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "b.flac".into(),
            path: std::path::PathBuf::from("/music/b.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();

    // B ends up Loading (its load supersedes A).
    let b_loading = wait_for(&snapshot, |s| s.state == PlaybackState::Loading);
    assert_eq!(b_loading.state, PlaybackState::Loading);

    // A subsequent stale property event is processed for the CURRENT
    // generation — it only refines position, never regresses the state
    // machine (Stopped/Loading/... edges are still enforced).
    let _ = tpos(0.0);
    actor.shutdown();
}

#[test]
fn property_event_updates_shared_position() {
    let (mut actor, snapshot, _props) = spawn_test(vec![tpos(7.5)]);
    let final_snap = wait_for(&snapshot, |s| {
        s.position == Some(7.5) || s.position.is_some()
    });
    assert_eq!(final_snap.position, Some(7.5));
    actor.shutdown();
}

// ------------------------------------------------------------------
// 音量的静音分叉 / 音频属性回读（用户报的"调大音量反而没声"）
// ------------------------------------------------------------------

/// Spawn an actor whose backend does the two things the real mpv backend
/// does and a scripted event list cannot express: it emits a `FileLoaded`
/// per load, and it **replays** mpv's current audio properties on every
/// subscription (mpv does exactly that the moment a property is observed).
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test_audio_readback(
    replay: (f64, bool, bool),
) -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
) {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(vec![]);
            b.props = props_join;
            b.file_loaded_on_load = true;
            b.audio_replay_on_observe = Some(replay);
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

#[test]
fn the_audio_controls_are_read_back_from_mpv_on_every_load() {
    // The snapshot must report what mpv *holds*, not what the actor last
    // commanded. With mpv replaying 音量 40 / 静音 / 暂停 on subscription,
    // the snapshot has to say exactly that — even though this very load
    // asked for playback. Before the audio controls were observed, the
    // actor's own opinion was final: UI 说未静音、实际静音（下一次「调大音量」
    // 正好撞上），UI 说在播、时钟却不走。
    let (mut actor, snapshot, props) = spawn_test_audio_readback((40.0, true, true));
    actor.send(load_temporary("/music/a.flac")).unwrap();

    let settled = wait_for(&snapshot, |s| s.state == PlaybackState::Paused && s.muted);
    assert_eq!(
        settled.state,
        PlaybackState::Paused,
        "mpv says still paused"
    );
    assert!(settled.muted, "mpv says muted");
    assert!(
        (settled.volume - 0.4).abs() < 1e-9,
        "mpv's 0–100 volume lands in the snapshot as 0.0–1.0 [got {:?}]",
        settled.volume
    );
    // The intent was still expressed to mpv — the divergence is resolved by
    // mpv's answer, not by the actor ignoring the load's intent.
    assert!(
        props
            .lock()
            .unwrap()
            .contains(&BackendProperty::Pause(false)),
        "a play-intent load still clears mpv's pause flag"
    );
    actor.shutdown();
}

#[test]
fn pause_only_rewrites_the_transport_states_it_can_express() {
    use PlaybackState as S;
    // The flag means Playing ⇄ Paused …
    assert_eq!(
        ActorLoop::<TestBackend>::transport_from_pause(S::Playing, true),
        Some(S::Paused)
    );
    assert_eq!(
        ActorLoop::<TestBackend>::transport_from_pause(S::Paused, false),
        Some(S::Playing)
    );
    // … a no-op when it already agrees …
    assert_eq!(
        ActorLoop::<TestBackend>::transport_from_pause(S::Playing, false),
        None
    );
    // … and nothing else. A late `pause=yes` echo must not turn an
    // `Ended`/`Stopped`/`Failed` snapshot into Paused — that would show a
    // finished track as loaded playback again.
    for state in [S::Ended, S::Stopped, S::Failed, S::Loading] {
        assert_eq!(
            ActorLoop::<TestBackend>::transport_from_pause(state, true),
            None,
            "pause must not rewrite {state:?}"
        );
        assert_eq!(
            ActorLoop::<TestBackend>::transport_from_pause(state, false),
            None,
            "pause must not rewrite {state:?}"
        );
    }
}

#[test]
fn transport_pause_and_play_write_the_mpv_pause_flag() {
    // Regression: 播放/暂停 only flipped the actor's own state flag — `pause`
    // was never written to mpv, so the control bar changed its icon while
    // the audio kept playing. Both commands must reach the backend.
    let (mut actor, snapshot, props) = spawn_test(vec![BackendEvent::FileLoaded]);
    actor.send(load_temporary("/music/a.flac")).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    actor.send(PlayerCommand::Pause).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
    actor.send(PlayerCommand::Play).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            // The `LoadTemporary` that got us here is a *play* intent, so
            // FileLoaded clears mpv's pause flag for the new file first.
            BackendProperty::Pause(false),
            BackendProperty::Pause(true),
            BackendProperty::Pause(false),
        ],
        "pause/resume must write mpv's `pause` property, not just the state flag"
    );
    actor.shutdown();
}

fn pending_effects_loop(events: Vec<BackendEvent>) -> ActorLoop<TestBackend> {
    let mut backend = TestBackend::new(events);
    backend.effects_ready = Some(Arc::new(AtomicBool::new(false)));
    let mut actor = ActorLoop::new(backend, snapshot_stub(), subscribers_stub());
    actor
        .effects
        .shared
        .submit(1, true, crate::effects::Payload::default())
        .unwrap();
    assert!(actor.effects.property("audio-params/samplerate", 48_000.0));
    assert!(actor.effects.property("audio-params/channel-count", 2.0));
    actor
}

/// Drive the real actor loop with a transition held pending until this
/// test observes its silent snapshot, then complete it and check release.
fn assert_effects_barrier_releases_after_confirmation(mut actor: ActorLoop<TestBackend>) {
    let snapshot = actor.snapshot.clone();
    let props = actor.backend.props.clone();
    let ready = actor.backend.effects_ready.as_ref().unwrap().clone();
    let effects = actor.effects.shared.clone();
    let (tx, rx) = mpsc::sync_channel(4);
    let thread = std::thread::spawn(move || actor.run(rx));
    let held = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
    let held_applied = effects.runtime().applied;
    let held_writes = props.lock().unwrap().clone();

    ready.store(true, Ordering::Release);
    let playing = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
    tx.send(PlayerCommand::Shutdown).unwrap();
    thread.join().unwrap();
    assert_eq!(held.state, PlaybackState::Paused);
    assert!(held_writes.contains(&BackendProperty::Pause(true)));
    assert_eq!(held_applied, crate::effects::AppliedState::Pending);
    assert!(held_writes
        .iter()
        .all(|prop| *prop != BackendProperty::Pause(false)));
    assert_eq!(playing.state, PlaybackState::Playing);
    assert_eq!(
        effects.runtime().applied,
        crate::effects::AppliedState::Applied
    );
    assert!(props
        .lock()
        .unwrap()
        .contains(&BackendProperty::Pause(false)));
}

#[test]
fn effects_pending_file_loaded_publishes_paused_until_the_first_sample_is_released() {
    let mut actor = pending_effects_loop(vec![
        BackendEvent::FileLoaded,
        BackendEvent::PropertyChanged {
            name: "pause".into(),
            value: 0.0,
        },
    ]);
    actor.state = PlaybackState::Loading;
    actor.publish(0);
    assert_effects_barrier_releases_after_confirmation(actor);
}

#[test]
fn rejected_file_loaded_pause_is_retried_before_effects_can_release() {
    let mut actor = pending_effects_loop(vec![BackendEvent::FileLoaded, BackendEvent::Shutdown]);
    actor.state = PlaybackState::Loading;
    actor.backend.fail_next_property = true;
    actor.backend.ready_after_pause_rejection = true;
    actor.publish(0);
    let (snapshots_tx, snapshots_rx) = mpsc::sync_channel(8);
    actor.subscribers.lock().unwrap().push(snapshots_tx);
    let (_tx, rx) = mpsc::sync_channel(1);
    actor.run(rx);
    assert_eq!(
        *actor.backend.props.lock().unwrap(),
        [BackendProperty::Pause(true), BackendProperty::Pause(false)]
    );
    assert_eq!(
        snapshots_rx
            .try_iter()
            .map(|snapshot| snapshot.state)
            .collect::<Vec<_>>(),
        [
            PlaybackState::Loading,
            PlaybackState::Paused,
            PlaybackState::Playing
        ]
    );
    assert!(!actor.effects_held);
}

#[test]
fn rejected_file_loaded_pause_remains_pending_until_hold_is_confirmed() {
    let mut actor = pending_effects_loop(vec![BackendEvent::FileLoaded]);
    actor.state = PlaybackState::Loading;
    actor.backend.fail_next_property = true;
    actor.publish(0);
    assert_effects_barrier_releases_after_confirmation(actor);
}

#[test]
fn effects_pending_play_and_toggle_publish_paused_until_confirmed() {
    for command in [PlayerCommand::Play, PlayerCommand::TogglePlayPause] {
        let mut actor = pending_effects_loop(vec![]);
        actor.state = PlaybackState::Paused;
        actor.intended_paused = true;
        assert!(actor.handle_command(command));
        assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Paused);
        assert!(actor.effects_held);
        assert!(!actor.intended_paused);
        assert_effects_barrier_releases_after_confirmation(actor);
    }
}

#[test]
fn effects_reconfiguration_publishes_paused_until_reconfirmed() {
    let mut actor = pending_effects_loop(vec![BackendEvent::AudioReconfigured]);
    actor
        .backend
        .effects_ready
        .as_ref()
        .unwrap()
        .store(true, Ordering::Release);
    assert!(actor.effects.tick(&mut actor.backend, true, false));
    actor
        .backend
        .effects_ready
        .as_ref()
        .unwrap()
        .store(false, Ordering::Release);
    actor.state = PlaybackState::Playing;
    actor.publish(0);
    assert_effects_barrier_releases_after_confirmation(actor);
}

#[test]
fn late_effects_reconfiguration_cannot_resume_an_ended_or_inactive_track() {
    for state in [
        PlaybackState::Ended,
        PlaybackState::Failed,
        PlaybackState::Stopped,
    ] {
        let mut actor = pending_effects_loop(vec![
            BackendEvent::AudioReconfigured,
            BackendEvent::Shutdown,
        ]);
        actor
            .backend
            .effects_ready
            .as_ref()
            .unwrap()
            .store(true, Ordering::Release);
        assert!(actor.effects.tick(&mut actor.backend, true, false));
        actor.state = state;
        // An old first-sample hold may outlive the transport event, but
        // neither that hold nor a late native rebuild can resume this track.
        actor.effects_held = true;
        actor.publish(0);
        let (_tx, rx) = mpsc::sync_channel(1);
        actor.run(rx);
        assert_eq!(actor.snapshot.read().unwrap().state, state);
        assert!(!actor
            .backend
            .props
            .lock()
            .unwrap()
            .contains(&BackendProperty::Pause(false)));
    }
}

#[test]
fn ended_event_cancels_an_effects_hold_before_a_late_reconfiguration() {
    let mut actor = pending_effects_loop(vec![
        BackendEvent::Ended,
        BackendEvent::AudioReconfigured,
        BackendEvent::Shutdown,
    ]);
    actor.state = PlaybackState::Paused;
    actor.effects_held = true;
    let (_tx, rx) = mpsc::sync_channel(1);
    actor.run(rx);
    assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Ended);
    assert!(!actor.effects_held);
    assert!(!actor
        .backend
        .props
        .lock()
        .unwrap()
        .contains(&BackendProperty::Pause(false)));
}

#[test]
fn effects_reconfiguration_keeps_loading_until_file_loaded() {
    let mut actor = pending_effects_loop(vec![
        BackendEvent::AudioReconfigured,
        BackendEvent::Shutdown,
    ]);
    actor
        .backend
        .effects_ready
        .as_ref()
        .unwrap()
        .store(true, Ordering::Release);
    actor.state = PlaybackState::Loading;
    actor.effects_held = true;
    actor.publish(0);
    let (_tx, rx) = mpsc::sync_channel(1);
    actor.run(rx);
    assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Loading);
    assert!(!actor
        .backend
        .props
        .lock()
        .unwrap()
        .contains(&BackendProperty::Pause(false)));
}

#[test]
fn rejected_reconfiguration_pause_stays_playing_and_retries_the_hold_before_release() {
    let mut actor = pending_effects_loop(vec![
        BackendEvent::AudioReconfigured,
        BackendEvent::Shutdown,
    ]);
    actor
        .backend
        .effects_ready
        .as_ref()
        .unwrap()
        .store(true, Ordering::Release);
    assert!(actor.effects.tick(&mut actor.backend, true, false));
    actor.backend.fail_all_properties = true;
    actor.state = PlaybackState::Playing;
    actor.publish(0);
    let (snapshots_tx, snapshots_rx) = mpsc::sync_channel(8);
    actor.subscribers.lock().unwrap().push(snapshots_tx);
    let (_tx, rx) = mpsc::sync_channel(1);
    actor.run(rx);
    assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Playing);
    assert!(actor.effects_held);
    assert!(actor.backend.props.lock().unwrap().is_empty());
    assert!(snapshots_rx
        .try_iter()
        .all(|snapshot| snapshot.state != PlaybackState::Paused));

    // Once writes recover, the loop must establish the pause hold, publish
    // its confirmed silent state, then release it after effects are ready.
    actor.backend.fail_all_properties = false;
    actor.backend.events.push_back(BackendEvent::Shutdown);
    let (_tx, rx) = mpsc::sync_channel(1);
    actor.run(rx);
    assert!(!actor.effects_held);
    assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Playing);
    assert_eq!(
        *actor.backend.props.lock().unwrap(),
        [BackendProperty::Pause(true), BackendProperty::Pause(false)]
    );
    assert_eq!(
        snapshots_rx
            .try_iter()
            .map(|snapshot| snapshot.state)
            .collect::<Vec<_>>(),
        [PlaybackState::Paused, PlaybackState::Playing]
    );
}

#[test]
fn pause_and_toggling_again_cancel_play_while_effects_hold_the_first_sample() {
    for command in [PlayerCommand::Pause, PlayerCommand::TogglePlayPause] {
        let mut actor = pending_effects_loop(vec![]);
        actor.state = PlaybackState::Paused;
        actor.intended_paused = true;
        assert!(actor.handle_command(PlayerCommand::TogglePlayPause));
        assert!(actor.effects_held);
        assert!(!actor.intended_paused);
        assert!(actor.handle_command(command));
        assert!(actor.intended_paused);
        assert!(!actor.effects_held);
        assert_eq!(actor.snapshot.read().unwrap().state, PlaybackState::Paused);
        assert_eq!(
            *actor.backend.props.lock().unwrap(),
            [BackendProperty::Pause(true), BackendProperty::Pause(true)]
        );
    }
}

#[test]
fn toggle_play_pause_writes_the_pause_flag() {
    let (mut actor, snapshot, props) = spawn_test_file_loaded_on_load();
    actor.send(load_temporary("/music/a.flac")).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    actor.send(PlayerCommand::TogglePlayPause).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
    // The space-bar hotkey path must reach mpv too (the leading write is the
    // load's play intent, re-asserted on FileLoaded).
    assert_eq!(
        wait_for_property_writes(&props, 2),
        vec![BackendProperty::Pause(false), BackendProperty::Pause(true)]
    );
    actor.shutdown();
}

#[test]
fn a_paused_load_publishes_paused_and_reasserts_the_pause_flag() {
    // 冷启动 prime / 会话恢复: the load is decoded but must never sound, and
    // the snapshot must say `Paused` — publishing `Playing` there made the UI
    // draw a pause icon and interpolate the progress bar forward for a silent
    // track (the reported "启动就是正在播放/点歌进度走了却没声音").
    let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
    let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

    actor
        .send(PlayerCommand::LoadLibrarySongPaused {
            song_id: echo_core::domain::ids::SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");

    let primed = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
    assert_eq!(
        primed.state,
        PlaybackState::Paused,
        "a primed load is decoded, not playing"
    );
    // The flag is written before the loadfile AND re-asserted on FileLoaded:
    // mpv keeps a runtime `pause` across `loadfile`, so the intent has to be
    // restated against the file that actually loaded.
    assert_eq!(
        props.lock().unwrap().clone(),
        vec![BackendProperty::Pause(true), BackendProperty::Pause(true)],
        "the paused load must hold mpv's `pause` flag on"
    );
    actor.shutdown();
}

#[test]
fn seek_queued_during_paused_load_runs_after_file_loaded() {
    let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
    let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

    actor
        .send(PlayerCommand::LoadLibrarySongPaused {
            song_id: echo_core::domain::ids::SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send paused load");
    actor
        .send(PlayerCommand::Seek(37.5))
        .expect("queue restore seek");

    let restored = wait_for(&snapshot, |s| {
        s.state == PlaybackState::Paused && s.position == Some(37.5)
    });
    assert_eq!(restored.position, Some(37.5));
    assert_eq!(
        props.lock().expect("property writes").clone(),
        vec![
            BackendProperty::Pause(true),
            BackendProperty::Pause(true),
            BackendProperty::Seek(37.5),
        ],
        "the restore seek must follow FileLoaded's pause write"
    );
    actor.shutdown();
}

#[test]
fn a_rejected_pause_write_on_a_play_load_keeps_the_unconfirmed_loading_state() {
    let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
    let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, true);
    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id: echo_core::domain::ids::SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Loading);
    actor.shutdown();
    assert_eq!(snapshot.read().unwrap().state, PlaybackState::Loading);
    assert!(props.lock().unwrap().is_empty());
}

#[test]
fn a_play_load_clears_the_pause_flag_a_primed_load_left_behind() {
    // The reported bug, in the actor: 冷启动 prime writes `pause=yes`, mpv
    // keeps it across `loadfile` (manual: "if any option is changed at
    // runtime … they are not reset when a new file is played"), so 点歌 by
    // `LoadLibrarySong` used to load a *silent* track while the snapshot said
    // `Playing` — 进度条在走，扬声器没声，只有播放控制栏的 播放 才真的出声。
    let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
    let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

    actor
        .send(PlayerCommand::LoadLibrarySongPaused {
            song_id: echo_core::domain::ids::SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");
    wait_for(&snapshot, |s| s.state == PlaybackState::Paused);

    actor
        .send(PlayerCommand::LoadLibrarySong {
            song_id: echo_core::domain::ids::SongId::new(),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .expect("send");
    let playing = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
    assert_eq!(playing.state, PlaybackState::Playing);

    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes.last(),
        Some(&BackendProperty::Pause(false)),
        "a play-intent load must clear mpv's `pause` flag, or the track stays \
         silent while the UI shows it playing [writes={writes:?}]"
    );
    actor.shutdown();
}
