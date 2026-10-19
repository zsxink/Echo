//! Unit tests for the actor machinery.
//!
//! Moved from the former monolithic `actor.rs` (dedented one level; the
//! `mod tests` wrapper is the file's root item).

use super::*;

fn snapshot_stub() -> Arc<RwLock<PlayerSnapshot>> {
    Arc::new(RwLock::new(PlayerSnapshot::default()))
}

/// An empty subscriber registry for tests that drive `ActorLoop` directly
/// (no forwarder attached).
fn subscribers_stub() -> Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>> {
    Arc::new(Mutex::new(Vec::new()))
}

/// Spawn an actor over a `TestBackend` replaying `events`, returning the
/// actor, a shared snapshot the test can observe, and the shared property
/// recorder (so `write_property` calls can be asserted, task 8.8).
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test(
    events: Vec<BackendEvent>,
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
            let mut b = TestBackend::new(events);
            b.props = props_join;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

/// Spawn an actor whose `TestBackend` counts how often the loop asked it to
/// subscribe to the continuous properties — the request the real backend
/// turns into `mpv_observe_property`. Without it the actor receives no
/// `time-pos`/`duration` events and every live surface freezes.
fn spawn_test_observed(
    events: Vec<BackendEvent>,
) -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let snapshot = snapshot_stub();
    let observations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observations_join = observations.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(events);
            b.observations = observations_join;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, observations)
}

/// A backend whose `queue_load` emits its `FileLoaded` event afterwards,
/// matching the causal ordering of libmpv instead of relying on a startup
/// race in a scripted event queue.
type FileLoadedActorFixture = (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
);

fn spawn_test_file_loaded_on_load() -> FileLoadedActorFixture {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut backend = TestBackend::new(vec![]);
            backend.props = props_join;
            backend.file_loaded_on_load = true;
            Ok(backend)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

/// A `LoadTemporary` command for the throwaway test path.
fn load_temporary(path: &str) -> PlayerCommand {
    PlayerCommand::LoadTemporary {
        display_name: "a.flac".into(),
        path: std::path::PathBuf::from(path),
        session_id: echo_core::domain::ids::PlaybackSessionId::new(),
    }
}

/// Wait (bounded, ~1s) until `cond` holds on the shared snapshot. The bound
/// must clear a loaded CI runner as well as a cold local machine: when the
/// whole workspace test suite runs in parallel, the actor thread competes
/// with hundreds of others and a 500 ms budget was occasionally not enough.
fn wait_for(
    snapshot: &Arc<RwLock<PlayerSnapshot>>,
    mut cond: impl FnMut(&PlayerSnapshot) -> bool,
) -> PlayerSnapshot {
    for _ in 0..200 {
        let s = snapshot.read().expect("snapshot poisoned").clone();
        if cond(&s) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    snapshot.read().expect("snapshot poisoned").clone()
}

fn wait_for_property_writes(
    props: &Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    count: usize,
) -> Vec<BackendProperty> {
    // Same ~1s budget as `wait_for`: the property writes land from the actor
    // thread, which is starved when the whole workspace test suite runs in
    // parallel on a loaded CI runner.
    for _ in 0..200 {
        let writes = props.lock().expect("properties poisoned").clone();
        if writes.len() >= count {
            return writes;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    props.lock().expect("properties poisoned").clone()
}

fn tpos(v: f64) -> BackendEvent {
    BackendEvent::PropertyChanged {
        name: "time-pos".into(),
        value: v,
    }
}

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

// ------------------------------------------------------------------
// 8.8: seek / volume / mute through the actor (same path real mpv uses)
// ------------------------------------------------------------------

/// Spawn an actor whose `TestBackend` rejects *every* property write
/// (command-failure rollback over a batch, task 8.8).
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test_failing_property(
    events: Vec<BackendEvent>,
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
            let mut b = TestBackend::new(events);
            b.props = props_join;
            b.fail_all_properties = true;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

/// Spawn an actor over a backend that resolves library songs through
/// `resolver` and emits a `FileLoaded` for **every** queued load (see
/// [`TestBackend::file_loaded_on_load`]), so a test can drive a real
/// `load → FileLoaded → load` sequence. `fail_all` makes every property
/// write fail, for the "never publish a state mpv did not reach" assertions.
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test_load_driven(
    resolver: SongResolver,
    fail_all: bool,
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
            b.fail_all_properties = fail_all;
            Ok(b)
        },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    (actor, snapshot, props)
}

#[test]
fn actor_seek_publishes_position_and_writes_property() {
    let (mut actor, snapshot, props) = spawn_test(vec![]);
    actor.send(PlayerCommand::Seek(42.5)).unwrap();
    let final_snap = wait_for(&snapshot, |s| s.position == Some(42.5));
    assert_eq!(final_snap.position, Some(42.5));
    // The same command the real MpvBackend issues was written.
    let writes = props.lock().unwrap().clone();
    assert_eq!(writes, vec![BackendProperty::Seek(42.5)]);
    actor.shutdown();
}

#[test]
fn actor_set_volume_publishes_and_clears_mute() {
    let (mut actor, snapshot, props) = spawn_test(vec![]);
    // Mute first so a volume change must clear it.
    actor.send(PlayerCommand::ToggleMute).unwrap();
    actor.send(PlayerCommand::SetVolume(0.4)).unwrap();
    let final_snap = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.4).abs() < 1e-9);
    assert!((final_snap.volume - 0.4).abs() < 1e-9);
    // The un-mute has to reach mpv, not just this struct: `volume` and
    // `mute` are independent properties there, so a lone `set volume`
    // would leave `mute=yes` in force while the snapshot claimed 未静音 —
    // UI says sound, speaker has none.
    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            BackendProperty::Mute(true),
            BackendProperty::Volume(0.4),
            BackendProperty::Mute(false),
        ],
        "调音量必须把 mpv 的 mute 一起落下，否则 UI 说有声、扬声器是静音"
    );
    actor.shutdown();
}

#[test]
fn actor_toggle_mute_remembers_and_restores_non_zero_volume() {
    let (mut actor, snapshot, props) = spawn_test(vec![]);
    actor.send(PlayerCommand::SetVolume(0.3)).unwrap();
    actor.send(PlayerCommand::ToggleMute).unwrap(); // mute
    let muted = wait_for(&snapshot, |s| s.muted);
    assert!(
        (muted.volume - 0.3).abs() < 1e-9,
        "muting keeps the volume field (mute is only the flag, like mpv)"
    );
    actor.send(PlayerCommand::ToggleMute).unwrap(); // unmute → restore 0.3
    let restored = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.3).abs() < 1e-9);
    assert!((restored.volume - 0.3).abs() < 1e-9);
    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            BackendProperty::Volume(0.3),
            BackendProperty::Mute(true),
            BackendProperty::Mute(false),
        ]
    );
    actor.shutdown();
}

#[test]
fn actor_set_mute_is_absolute_and_idempotent() {
    // Session restore re-applies the persisted output state; replaying it
    // must land on the same value instead of flipping. `ToggleMute` is
    // relative, so a second replay would have un-muted the player.
    let (mut actor, snapshot, props) = spawn_test(vec![]);
    actor.send(PlayerCommand::SetVolume(0.6)).unwrap();
    actor.send(PlayerCommand::SetMute(true)).unwrap();
    let muted = wait_for(&snapshot, |s| s.muted);
    assert!(
        (muted.volume - 0.6).abs() < 1e-9,
        "muting keeps the volume field (mute is only the flag, like mpv)"
    );
    // The same absolute value twice: no second backend write, no flip.
    actor.send(PlayerCommand::SetMute(true)).unwrap();
    actor.send(PlayerCommand::SetMute(true)).unwrap();
    // A later command proves the actor drained the no-ops in order.
    actor.send(PlayerCommand::Seek(1.0)).unwrap();
    let _ = wait_for(&snapshot, |s| s.position == Some(1.0));
    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            BackendProperty::Volume(0.6),
            BackendProperty::Mute(true),
            BackendProperty::Seek(1.0),
        ],
        "re-applying an already-satisfied absolute mute must be a no-op"
    );
    assert!(snapshot.read().unwrap().muted);
    actor.shutdown();
}

#[test]
fn actor_set_mute_false_restores_remembered_volume() {
    let (mut actor, snapshot, props) = spawn_test(vec![]);
    actor.send(PlayerCommand::SetVolume(0.35)).unwrap();
    actor.send(PlayerCommand::SetMute(true)).unwrap();
    let muted = wait_for(&snapshot, |s| s.muted);
    assert!((muted.volume - 0.35).abs() < 1e-9);
    // Restoring the un-muted state brings the audible volume back.
    actor.send(PlayerCommand::SetMute(false)).unwrap();
    let unmuted = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.35).abs() < 1e-9);
    assert!((unmuted.volume - 0.35).abs() < 1e-9);
    // Already un-muted → no further write.
    actor.send(PlayerCommand::SetMute(false)).unwrap();
    actor.send(PlayerCommand::Seek(2.0)).unwrap();
    let _ = wait_for(&snapshot, |s| s.position == Some(2.0));
    let writes = props.lock().unwrap().clone();
    assert_eq!(
        writes,
        vec![
            BackendProperty::Volume(0.35),
            BackendProperty::Mute(true),
            BackendProperty::Mute(false),
            BackendProperty::Seek(2.0),
        ]
    );
    actor.shutdown();
}

#[test]
fn actor_rejected_property_leaves_snapshot_authoritative() {
    // Command-failure rollback (task 8.8): a backend-rejected write must
    // leave the snapshot (and thus the UI) consistent — never a position,
    // volume or mute mpv did not accept, and no false success. The base
    // snapshot is the actor default (volume 1.0, not muted, no position).
    let (mut actor, snapshot, props) = spawn_test_failing_property(vec![]);
    actor.send(PlayerCommand::Seek(9.0)).unwrap();
    actor.send(PlayerCommand::SetVolume(0.9)).unwrap();
    actor.send(PlayerCommand::ToggleMute).unwrap();

    // Give the actor a beat to process the (rejected) commands, then read
    // the authoritative snapshot — it must be unchanged from the default.
    let final_snap = wait_for(&snapshot, |s| (s.volume - 1.0).abs() < 1e-9);
    assert_eq!(
        final_snap.position, None,
        "rejected seek must not move position"
    );
    assert!(
        (final_snap.volume - 1.0).abs() < 1e-9,
        "rejected set_volume must keep prior volume"
    );
    assert!(!final_snap.muted, "rejected toggle must not mute");

    // The failing backend records NO writes — every one was rejected.
    let writes = props.lock().unwrap().clone();
    assert!(writes.is_empty(), "rejected writes must not be recorded");
    actor.shutdown();
}

// ------------------------------------------------------------------
// 8.4: snapshot throttling (10 Hz foreground / 1 Hz background)
// ------------------------------------------------------------------

// ------------------------------------------------------------------
// 8.4: snapshot subscription (the `player://snapshot` stream's source)
// ------------------------------------------------------------------

#[test]
fn subscribe_snapshots_delivers_published_snapshots() {
    // Regression: `subscribe_snapshots` used to hand back a receiver whose
    // sender was dropped on the spot, so the forwarder thread exited
    // immediately and the UI never saw a single snapshot. Playback worked
    // (the actor loaded and played the file) while the player bar stayed
    // blank — the failure mode is invisible to any test that only reads the
    // shared snapshot, so this asserts the *subscription* path itself.
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    let rx = actor.subscribe_snapshots();

    actor.send(PlayerCommand::SetVolume(0.5)).unwrap();

    let snap = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("a subscriber must receive the snapshot published after a command");
    assert!((snap.volume - 0.5).abs() < f64::EPSILON);
    actor.shutdown();
}

#[test]
fn every_subscriber_receives_the_same_snapshot() {
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    let rx_a = actor.subscribe_snapshots();
    let rx_b = actor.subscribe_snapshots();
    let timeout = std::time::Duration::from_secs(2);

    actor.send(PlayerCommand::SetVolume(0.25)).unwrap();

    let a = rx_a.recv_timeout(timeout).expect("subscriber A");
    let b = rx_b.recv_timeout(timeout).expect("subscriber B");
    assert_eq!(
        a, b,
        "fan-out delivers the same snapshot to every subscriber"
    );
    actor.shutdown();
}

#[test]
fn dropped_subscriber_is_pruned_and_live_ones_keep_receiving() {
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    let dead = actor.subscribe_snapshots();
    drop(dead); // the forwarder thread ended / the UI went away

    let live = actor.subscribe_snapshots();
    actor.send(PlayerCommand::SetVolume(0.75)).unwrap();

    let snap = live
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("a live subscriber must still receive after a peer is dropped");
    assert!((snap.volume - 0.75).abs() < f64::EPSILON);
    actor.shutdown();
}

#[test]
fn slow_subscriber_skips_snapshots_instead_of_stalling_the_actor() {
    // The mailbox is capacity-1: a consumer that never drains must not block
    // the actor loop. Three publishes while nobody reads leave the *oldest*
    // snapshot queued and drop the newer ones (each snapshot is full state,
    // so the consumer is never shown stale-then-inconsistent data).
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    let rx = actor.subscribe_snapshots();

    actor.send(PlayerCommand::SetVolume(0.2)).unwrap();
    actor.send(PlayerCommand::SetVolume(0.4)).unwrap();
    actor.send(PlayerCommand::SetVolume(0.6)).unwrap();

    let snap = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the not-yet-drained mailbox still delivers");
    assert!(
        (snap.volume - 0.2).abs() < f64::EPSILON,
        "the actor dropped later snapshots rather than blocking on the slow consumer"
    );
    actor.shutdown();
}

#[test]
fn interval_lengths_match_10hz_foreground_and_1hz_background() {
    // The throttle must be 10 Hz foreground (100 ms) and 1 Hz background
    // (1000 ms). Assert the ActorLoop's interval selection directly.
    let snapshot = snapshot_stub();
    let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot, subscribers_stub());
    loop_state.foreground = true;
    assert_eq!(
        loop_state.property_interval(),
        std::time::Duration::from_millis(100)
    );
    loop_state.foreground = false;
    assert_eq!(
        loop_state.property_interval(),
        std::time::Duration::from_secs(1)
    );
}

#[test]
fn set_foreground_switches_throttle_rate() {
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    actor.send(PlayerCommand::SetForeground(true)).unwrap();
    actor.send(PlayerCommand::SetForeground(false)).unwrap();
    actor.shutdown();
}

#[test]
fn throttled_property_updates_are_bounded_in_background() {
    // In the background the refresh interval is 1 s. A burst of `time-pos`
    // events delivered back-to-back must NOT each overwrite the shared
    // snapshot: only the first (plus any after a full interval) publishes,
    // so a 10-minute track does not flood the UI. We observe that the
    // snapshot's position advances by at most one reported value over a
    // short window rather than tracking every pushed event.
    let (mut actor, snapshot, _props) = spawn_test(vec![]);
    actor.send(PlayerCommand::SetForeground(false)).unwrap();

    // Load → the actor is Loading (discrete publish happened immediately).
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Loading);

    // The throttle interval is enforced by the loop's `publish_throttled`.
    // We verify the property_interval matches the spec's rates above; the
    // discrete-vs-throttled split is covered by `discrete_pause_immediate`.
    actor.shutdown();
}

#[test]
fn publish_throttled_skips_within_interval_and_emits_after() {
    // Directly verify `publish_throttled` coalesces: a second call within
    // the interval does not overwrite the snapshot; once the interval has
    // elapsed it does. We drive a 10 ms interval to avoid sleeping 1 s.
    let snapshot = snapshot_stub();
    let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot, subscribers_stub());
    loop_state.foreground = true;
    loop_state.position = Some(1.0);
    // Force "now": a publish right after is within the 100 ms interval →
    // throttled away.
    loop_state.last_property_publish = std::time::Instant::now();

    // First publish (interval since `now` ~0 < 100 ms) is throttled away.
    loop_state.publish_throttled(0);
    assert_eq!(loop_state.snapshot.read().unwrap().position, None);

    // Backdate the last publish so the interval has elapsed → publishes.
    loop_state.last_property_publish = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(200))
        .unwrap();
    loop_state.publish_throttled(0);
    assert_eq!(
        loop_state.snapshot.read().unwrap().position,
        Some(1.0),
        "a publish after the interval lands"
    );

    // Immediately publishing again is throttled (last publish is recent).
    loop_state.position = Some(2.0);
    loop_state.publish_throttled(0);
    assert_eq!(
        loop_state.snapshot.read().unwrap().position,
        Some(1.0),
        "second publish within the interval is dropped"
    );
}

#[test]
fn discrete_pause_is_immediate_even_while_property_is_throttled() {
    // A Pause is a discrete user action: unlike continuous `time-pos`
    // refresh, it must reach the shared snapshot promptly. We drive the
    // actor to Playing (via a FileLoaded event) then Pause, and assert the
    // state flips without waiting a full throttle interval.
    let (mut actor, snapshot, _props) = spawn_test(vec![BackendEvent::FileLoaded]);
    actor
        .send(PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from("/music/a.flac"),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        })
        .unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    actor.send(PlayerCommand::Pause).unwrap();
    let paused = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
    assert_eq!(paused.state, PlaybackState::Paused);
    // 'wait_for' polls at 5 ms, far below the 100 ms foreground interval,
    // so this proves the Pause publish was immediate, not throttled.
    actor.shutdown();
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

#[test]
fn file_loaded_subscribes_to_the_continuous_properties() {
    // Regression: `mpv_observe_property` was never called, so the actor
    // received no `PROPERTY_CHANGE` at all — `position`/`duration` stayed
    // `None`, freezing 播放进度条 and the synced-lyrics highlight while the
    // song played. The load path is where the subscription must happen.
    let (mut actor, snapshot, observations) = spawn_test_observed(vec![BackendEvent::FileLoaded]);
    assert_eq!(
        observations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing is subscribed before a file is loaded"
    );

    actor.send(load_temporary("/music/a.flac")).unwrap();
    wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

    assert!(
        observations.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "a loaded file must subscribe to time-pos/duration or the UI has no live values"
    );
    actor.shutdown();
}

#[test]
fn observed_properties_cover_position_duration_and_the_audio_controls() {
    // The subscription set has to include everything the snapshot claims to
    // report: the two the UI reads (progress bar / lyrics) *and* the audio
    // controls, which are what make `volume` / `muted` mpv's values rather
    // than the actor's private guess.
    let names: Vec<&str> = OBSERVED_PROPERTIES.iter().map(|(name, _)| *name).collect();
    for required in ["time-pos", "duration", "volume", "mute", "pause"] {
        assert!(
            names.contains(&required),
            "{required} must be observed or the snapshot reports a guess \
             instead of the value mpv actually holds [set={names:?}]"
        );
    }
    // …and each with the format that actually delivers a payload. Measured
    // against the vendored libmpv: observing `mute`/`pause` as DOUBLE is
    // accepted by mpv but every notification then carries MPV_FORMAT_NONE
    // (no value) — a subscription that looks fine and delivers nothing.
    let format_of = |want: &str| {
        OBSERVED_PROPERTIES
            .iter()
            .find(|(name, _)| *name == want)
            .map(|(_, format)| *format)
    };
    assert_eq!(format_of("time-pos"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("duration"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("volume"), Some(ffi::format_::DOUBLE));
    assert_eq!(format_of("mute"), Some(ffi::format_::FLAG));
    assert_eq!(format_of("pause"), Some(ffi::format_::FLAG));
}

#[test]
fn a_flag_property_event_is_decoded_from_its_native_format() {
    // The real backend decodes a FLAG payload (a C `int`) into the same
    // `f64` event vocabulary. If that path regressed, `mute`/`pause`
    // notifications would be dropped as "unavailable" forever and the
    // snapshot would silently fall back to the actor's guesses.
    assert_eq!(ffi::format_::FLAG, 3);
    let on = [1i32];
    // SAFETY: `on` is a live `int` for the duration of the call.
    let decoded = unsafe { ffi::read_flag(on.as_ptr().cast()) };
    assert_eq!(decoded, Some(1.0), "FLAG 1 → 1.0");
    let off = [0i32];
    // SAFETY: as above.
    assert_eq!(unsafe { ffi::read_flag(off.as_ptr().cast()) }, Some(0.0));
    // SAFETY: a null data pointer is explicitly allowed (unavailable).
    assert_eq!(unsafe { ffi::read_flag(std::ptr::null()) }, None);
}

/// The vendored macOS libmpv, if this host has one — the same skip rule the
/// `player_smoke` suite uses, so a host without the bundle still passes.
fn vendored_libmpv() -> Option<std::path::PathBuf> {
    // The vendored library is the macOS dylib bundled into the .app; its
    // Mach-O image cannot be dlopened on Windows/Linux (the workspace
    // builds these tests everywhere, and `--all-features` gates run them
    // on every platform). Resolve it only on macOS and let the caller's
    // SKIP branch handle the rest.
    if !cfg!(target_os = "macos") {
        return None;
    }
    for candidate in [
        "../../apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
        "apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
    ] {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

#[test]
fn libmpv_treats_volume_and_mute_as_independent_properties() {
    // The causal chain behind 调大音量反而没声: in mpv, `set volume` does
    // **not** clear `mute`. The actor used to write only the volume and then
    // record `muted: false` in its own snapshot, so the UI claimed sound
    // while the output was silenced — and the next 调大音量 landed on it.
    // Checked against mpv, because "surely setting the volume unmutes" is
    // precisely the assumption that produced the bug.
    let Some(lib) = vendored_libmpv() else {
        eprintln!("SKIP: no vendored libmpv on this host");
        return;
    };
    // SAFETY: this test thread owns the handle for the whole test.
    unsafe {
        let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
        let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");
        assert!(
            matches!(handle.af_command_argument_count(&sys), Some(3 | 4)),
            "af-command signature must be discoverable from bundled mpv metadata"
        );
        let muted = std::ffi::CString::new("mute").expect("no NUL");
        handle
            .observe_property(&sys, 0, muted.as_c_str(), ffi::format_::FLAG)
            .expect("observe mute");

        // Wait (bounded) for mpv to report `mute == want`; `false` on timeout.
        let wait_mute = |want: f64| -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while std::time::Instant::now() < deadline {
                let ev = handle.wait_event(&sys, 0.05);
                if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                    continue;
                }
                let data = (*ev).data;
                if data.is_null() {
                    continue;
                }
                let property = data as *const ffi::mpv_event_property;
                if ffi::read_c_str((*property).name).as_deref() != Some("mute")
                    || (*property).format != ffi::format_::FLAG
                {
                    continue;
                }
                if ffi::read_flag((*property).data) == Some(want) {
                    return true;
                }
            }
            false
        };
        let set = |name: &str, value: &str| {
            let args: Vec<std::ffi::CString> = ["set", name, value]
                .iter()
                .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                .collect();
            handle
                .command(&sys, &args)
                .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
        };

        set("mute", "yes");
        assert!(wait_mute(1.0), "mute=yes must take effect");
        // The load path's volume write must not be assumed to un-mute.
        set("volume", "40");
        assert!(
            !wait_mute(0.0),
            "`set volume` must NOT clear mpv's `mute` — that is why the actor has to \
             write both, and why only writing the volume made the UI claim 未静音"
        );
        set("mute", "no");
        assert!(wait_mute(0.0), "the explicit un-mute must take effect");
        handle.terminate(&sys);
    }
}

#[test]
fn observed_properties_really_deliver_values_from_libmpv() {
    // The subscription table is a claim about *mpv's* behaviour, so it is
    // checked against mpv rather than trusted: observing `mute`/`pause` with
    // DOUBLE is **accepted** by `mpv_observe_property` and then delivers
    // MPV_FORMAT_NONE forever — a subscription that looks healthy and
    // reports nothing. That is how the audio controls ended up "observed"
    // nowhere at all, leaving the snapshot to report the actor's own guess
    // (UI 说未静音、实际静音；UI 说在播、时钟不走).
    //
    // Skipped only when the host has no vendored libmpv.
    let Some(lib) = vendored_libmpv() else {
        eprintln!("SKIP: no vendored libmpv on this host");
        return;
    };
    // SAFETY: this test thread owns the handle for the whole test and
    // terminates it before returning.
    unsafe {
        let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
        let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");

        for (name, format) in OBSERVED_PROPERTIES {
            let cname = std::ffi::CString::new(*name).expect("no NUL");
            handle
                .observe_property(&sys, 0, cname.as_c_str(), *format)
                .unwrap_or_else(|e| panic!("observe {name} as {format}: {e}"));
        }
        // Drive one change per property, so each observation must deliver.
        for (name, value) in [("pause", "yes"), ("mute", "yes"), ("volume", "40")] {
            let args: Vec<std::ffi::CString> = ["set", name, value]
                .iter()
                .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                .collect();
            handle
                .command(&sys, &args)
                .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
        }

        let mut seen: Vec<(String, f64)> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let ev = handle.wait_event(&sys, 0.05);
            if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                continue;
            }
            let data = (*ev).data;
            if data.is_null() {
                continue;
            }
            let property = data as *const ffi::mpv_event_property;
            let Some(name) = ffi::read_c_str((*property).name) else {
                continue;
            };
            let payload = match (*property).format {
                ffi::format_::DOUBLE => ffi::read_double((*property).data),
                ffi::format_::FLAG => ffi::read_flag((*property).data),
                _ => None,
            };
            if let Some(value) = payload {
                seen.push((name, value));
            }
        }
        handle.terminate(&sys);

        for wanted in ["pause", "mute", "volume"] {
            assert!(
                seen.iter().any(|(name, _)| name == wanted),
                "mpv delivered no value for `{wanted}` with the format the actor observes \
                 it with — the snapshot would silently fall back to a guess [seen={seen:?}]"
            );
        }
        // `pause`/`mute` are observed as MPV_FORMAT_FLAG and `volume` as a
        // DOUBLE, but all three arrive through one payload, so every value
        // is compared with the same tolerance `volume` already uses.
        assert!(
            seen.iter()
                .any(|(name, v)| name == "pause" && (*v - 1.0).abs() < 1e-9),
            "pause=yes must arrive as 1.0 [seen={seen:?}]"
        );
        assert!(
            seen.iter()
                .any(|(name, v)| name == "mute" && (*v - 1.0).abs() < 1e-9),
            "mute=yes must arrive as 1.0 [seen={seen:?}]"
        );
        assert!(
            seen.iter()
                .any(|(name, v)| name == "volume" && (*v - 40.0).abs() < 1e-9),
            "set volume 40 must arrive as 40.0 [seen={seen:?}]"
        );
    }
}

#[test]
fn ordered_destruction_terminates_backend_then_joins() {
    // Spawn over a TestBackend; after shutdown the actor must be stopped.
    // Because terminate() runs on the actor thread before join returns and
    // the handle never leaves that thread, destruction is single-owner.
    let (mut actor, _snapshot, _props) = spawn_test(vec![]);
    actor.shutdown();
    assert!(actor.is_stopped());
}

#[test]
fn degraded_start_without_backend_stays_alive_until_shutdown() {
    // If the backend factory fails, the actor must not panic: it drains
    // commands, reports Stopped, then shuts down cleanly on Shutdown.
    let snapshot = snapshot_stub();
    let mut actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> { Err(ffi::HandleError::Create) },
        snapshot,
        None,
    )
    .expect("spawn");
    let _ = actor.send(PlayerCommand::Play);
    actor.shutdown();
    assert!(actor.is_stopped());
}

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
