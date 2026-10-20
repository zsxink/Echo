//! 音量分叉与音频属性回读、8.8 seek / volume / mute through the actor.

#[allow(clippy::wildcard_imports)]
use super::*;

// ------------------------------------------------------------------
// 8.8: seek / volume / mute through the actor (same path real mpv uses)
// ------------------------------------------------------------------

/// Spawn an actor whose `TestBackend` rejects *every* property write
/// (command-failure rollback over a batch, task 8.8).

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
