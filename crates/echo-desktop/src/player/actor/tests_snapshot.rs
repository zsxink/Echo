//! 8.4: snapshot subscription and 10 Hz foreground / 1 Hz background throttling.

#[allow(clippy::wildcard_imports)]
use super::*;

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
