//! In-process test double for [`PlayerPort`] (task 8.1).
//!
//! `FakePlayer` processes every [`PlayerCommand`] immediately on the calling
//! thread, updating its internal state and publishing a [`PlayerSnapshot`] to
//! all active subscribers. This lets coordinator, queue, statistics and
//! platform-control tests verify their logic without loading libmpv.
//!
//! **Behavioral contract:**
//! - `LoadLibrarySong` / `LoadTemporary` transition to `Playing` (the fake
//!   never blocks on I/O).
//! - `Play` / `Pause` / `TogglePlayPause` respect the [`PlaybackState`]
//!   transition edges.
//! - `Seek` / `SetVolume` / `ToggleMute` update the snapshot immediately.
//! - `Next` / `Previous` set state to `Ended` so the coordinator can
//!   advance the queue; the coordinator is responsible for issuing a new
//!   `Load*` command.
//! - `Stop` transitions to `Stopped`; `Shutdown` transitions to `Stopped`
//!   and marks the actor as shut down — subsequent `send()` calls return
//!   `Err(PlayerError::ActorClosed)`.
//!
//! Tests can call [`FakePlayer::set_state`] / [`FakePlayer::set_position`]
//! to simulate backend-driven state changes (e.g. end-of-file, seek
//! completion) without going through the command channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use echo_core::domain::ids::{PlaybackSessionId, QueueEntryId};
use echo_core::domain::state::PlaybackState;

use super::port::{PlayMode, PlayerCommand, PlayerError, PlayerPort, PlayerSnapshot};

/// In-process fake player for tests. Implements [`PlayerPort`] by processing
/// commands synchronously and maintaining a mutable snapshot.
pub struct FakePlayer {
    inner: Arc<Mutex<FakeInner>>,
    shutdown: Arc<AtomicBool>,
}

struct FakeInner {
    snapshot: PlayerSnapshot,
    /// Registered subscriber senders. Dead subscribers are silently pruned
    /// on each publish.
    subscribers: Vec<mpsc::Sender<PlayerSnapshot>>,
    /// The last `SongId` loaded via `LoadLibrarySong` (for test assertions).
    last_loaded_song: Option<echo_core::domain::ids::SongId>,
    /// The last `PlaybackSessionId` used in a `Load*` command.
    last_session: Option<PlaybackSessionId>,
    /// The last non-zero volume, so unmuting restores it (task 8.8). Mirrors
    /// the actor's [`crate::player::actor`] semantics so the fake and real mpv
    /// produce identical UI snapshots.
    last_nonzero_volume: f64,
    /// When true, the next property write (`Seek` / `SetVolume` / `ToggleMute`)
    /// is rejected — the snapshot must remain untouched (authoritative
    /// rollback, task 8.8). Mirrors the actor's `TestBackend` failure knob.
    fail_next_property: bool,
}

impl FakePlayer {
    /// Create a new fake in `Stopped` state with default snapshot values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeInner {
                snapshot: PlayerSnapshot::default(),
                subscribers: Vec::new(),
                last_loaded_song: None,
                last_session: None,
                last_nonzero_volume: 1.0,
                fail_next_property: false,
            })),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    // -- Test helpers -------------------------------------------------------

    /// The last `SongId` the coordinator asked to load, if any.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn last_loaded_song(&self) -> Option<echo_core::domain::ids::SongId> {
        self.inner.lock().expect("fake poisoned").last_loaded_song
    }

    /// The last `PlaybackSessionId` used in a `Load*` command.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn last_session(&self) -> Option<PlaybackSessionId> {
        self.inner.lock().expect("fake poisoned").last_session
    }

    /// Manually set the player state (simulate a backend-driven transition
    /// like end-of-file or decode error).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn set_state(&self, state: PlaybackState) {
        let snap = {
            let mut guard = self.inner.lock().expect("fake poisoned");
            guard.snapshot.state = state;
            guard.snapshot.clone()
        };
        self.publish_snapshot(&snap);
    }

    /// Manually set the playback position (simulate time advancing).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn set_position(&self, position: f64) {
        let snap = {
            let mut guard = self.inner.lock().expect("fake poisoned");
            guard.snapshot.position = Some(position);
            guard.snapshot.clone()
        };
        self.publish_snapshot(&snap);
    }

    /// Set the playback mode.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn set_mode(&self, mode: PlayMode) {
        let snap = {
            let mut guard = self.inner.lock().expect("fake poisoned");
            guard.snapshot.mode = mode;
            guard.snapshot.clone()
        };
        self.publish_snapshot(&snap);
    }

    /// Publish a snapshot to all live subscribers (test helper for simulating
    /// actor-driven pushes).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn push_snapshot(&self) {
        let snap = {
            let guard = self.inner.lock().expect("fake poisoned");
            guard.snapshot.clone()
        };
        self.publish_snapshot(&snap);
    }

    /// Drain all pending snapshots from a subscriber receiver.
    #[must_use]
    pub fn drain_snapshots(rx: &mpsc::Receiver<PlayerSnapshot>) -> Vec<PlayerSnapshot> {
        rx.try_iter().collect()
    }

    /// Arm the fake to reject the *next* property write (`Seek` / `SetVolume`
    /// / `ToggleMute`), so the coordinator returns without committing the
    /// snapshot — the authoritative rollback path (task 8.8).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn fail_next_property(&self) {
        self.inner.lock().expect("fake poisoned").fail_next_property = true;
    }
}

impl Default for FakePlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerPort for FakePlayer {
    fn send(&self, cmd: PlayerCommand) -> Result<(), PlayerError> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(PlayerError::ActorClosed);
        }

        // Process the command under the lock, then release before publishing
        // so the guard is dropped explicitly (clippy::significant_drop).
        let snap = {
            let mut guard = self.inner.lock().expect("fake poisoned");

            match cmd {
                PlayerCommand::LoadLibrarySong {
                    song_id,
                    session_id,
                } => {
                    guard.last_loaded_song = Some(song_id);
                    guard.last_session = Some(session_id);
                    guard.snapshot.state = PlaybackState::Playing;
                    guard.snapshot.current_item = Some(QueueEntryId::new());
                    guard.snapshot.queue_len = 1;
                    guard.snapshot.position = Some(0.0);
                    guard.snapshot.duration = None;
                }
                PlayerCommand::LoadTemporary { session_id, .. } => {
                    guard.last_session = Some(session_id);
                    guard.snapshot.state = PlaybackState::Playing;
                    guard.snapshot.current_item = Some(QueueEntryId::new());
                    guard.snapshot.queue_len = 1;
                    guard.snapshot.position = Some(0.0);
                    guard.snapshot.duration = None;
                }
                PlayerCommand::Play => {
                    if guard
                        .snapshot
                        .state
                        .can_transition_to(PlaybackState::Playing)
                    {
                        guard.snapshot.state = PlaybackState::Playing;
                    }
                }
                PlayerCommand::Pause => {
                    if guard
                        .snapshot
                        .state
                        .can_transition_to(PlaybackState::Paused)
                    {
                        guard.snapshot.state = PlaybackState::Paused;
                    }
                }
                PlayerCommand::TogglePlayPause => match guard.snapshot.state {
                    PlaybackState::Playing => {
                        guard.snapshot.state = PlaybackState::Paused;
                    }
                    PlaybackState::Paused => {
                        guard.snapshot.state = PlaybackState::Playing;
                    }
                    _ => {}
                },
                PlayerCommand::Next | PlayerCommand::Previous => {
                    // The coordinator handles queue advancement; the actor just
                    // signals that the current item ended.
                    guard.snapshot.state = PlaybackState::Ended;
                    guard.snapshot.position = None;
                }
                PlayerCommand::Seek(pos) => {
                    // Authoritative rollback (task 8.8): a rejected seek must
                    // leave the snapshot untouched, mirroring the actor.
                    if guard.fail_next_property {
                        guard.fail_next_property = false;
                        return Ok(());
                    }
                    guard.snapshot.position = Some(pos);
                }
                PlayerCommand::SetVolume(vol) => {
                    if guard.fail_next_property {
                        guard.fail_next_property = false;
                        return Ok(());
                    }
                    let clamped = vol.clamp(0.0, 1.0);
                    // Mirrors the actor: a non-zero choice re-members the
                    // recent non-zero value; mute is cleared only when the user
                    // picks an audible level (a zero choice leaves mute alone).
                    if clamped > 0.0 {
                        guard.last_nonzero_volume = clamped;
                    }
                    guard.snapshot.volume = clamped;
                    guard.snapshot.muted = guard.snapshot.muted && clamped == 0.0;
                }
                PlayerCommand::ToggleMute => {
                    if guard.fail_next_property {
                        guard.fail_next_property = false;
                        return Ok(());
                    }
                    // Mirrors the actor exactly: muting keeps the volume field
                    // unchanged (mute is only the `muted` flag) but remembers
                    // the audible volume; unmuting restores the recent non-zero
                    // volume. This keeps FakePlayer and real mpv snapshots
                    // identical (task 8.8).
                    let target = !guard.snapshot.muted;
                    if target {
                        // muting: remember the current audible volume.
                        if guard.snapshot.volume > 0.0 {
                            guard.last_nonzero_volume = guard.snapshot.volume;
                        }
                    } else {
                        // unmuting: restore the remembered non-zero volume.
                        guard.snapshot.volume = guard.last_nonzero_volume;
                    }
                    guard.snapshot.muted = target;
                }
                PlayerCommand::SetForeground(_) => {
                    // The fake has no throttle; foreground is a no-op.
                }
                PlayerCommand::Stop => {
                    guard.snapshot.state = PlaybackState::Stopped;
                    guard.snapshot.position = None;
                    guard.snapshot.current_item = None;
                }
                PlayerCommand::Shutdown => {
                    guard.snapshot.state = PlaybackState::Stopped;
                    guard.snapshot.position = None;
                    guard.snapshot.current_item = None;
                    drop(guard);
                    self.shutdown.store(true, Ordering::Release);
                    return Ok(());
                }
            }

            guard.snapshot.clone()
        }; // guard dropped here

        // Publish to subscribers without holding the inner lock.
        self.publish_snapshot(&snap);
        Ok(())
    }

    fn snapshot(&self) -> PlayerSnapshot {
        self.inner.lock().expect("fake poisoned").snapshot.clone()
    }

    fn subscribe_snapshots(&self) -> mpsc::Receiver<PlayerSnapshot> {
        let (tx, rx) = mpsc::channel();
        let mut guard = self.inner.lock().expect("fake poisoned");
        guard.subscribers.push(tx);
        rx
    }
}

impl FakePlayer {
    /// Publish a snapshot to all live subscribers without holding the inner
    /// lock. Subscribers are taken out, filtered to the live ones, and put
    /// back — the lock is never held across a `send`.
    fn publish_snapshot(&self, snap: &PlayerSnapshot) {
        let subscribers = {
            let mut guard = self.inner.lock().expect("fake poisoned");
            std::mem::take(&mut guard.subscribers)
        };
        let live: Vec<_> = subscribers
            .into_iter()
            .filter(|tx| tx.send(snap.clone()).is_ok())
            .collect();
        let mut guard = self.inner.lock().expect("fake poisoned");
        guard.subscribers.extend(live);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::domain::ids::SongId;
    use echo_core::domain::state::PlaybackState;

    fn song() -> SongId {
        SongId::new()
    }

    #[test]
    fn initial_snapshot_is_stopped() {
        let fake = FakePlayer::new();
        let snap = fake.snapshot();
        assert_eq!(snap.state, PlaybackState::Stopped);
        assert!((snap.volume - 1.0).abs() < f64::EPSILON);
        assert!(!snap.muted);
        assert!(snap.current_item.is_none());
        assert_eq!(snap.queue_len, 0);
        assert_eq!(snap.mode, PlayMode::Sequential);
    }

    #[test]
    fn load_library_song_transitions_to_playing() {
        let fake = FakePlayer::new();
        let s = song();
        let session = PlaybackSessionId::new();

        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: s,
            session_id: session,
        })
        .unwrap();

        let snap = fake.snapshot();
        assert_eq!(snap.state, PlaybackState::Playing);
        assert!(snap.current_item.is_some());
        assert_eq!(fake.last_loaded_song(), Some(s));
        assert_eq!(fake.last_session(), Some(session));
    }

    #[test]
    fn load_temporary_transitions_to_playing() {
        let fake = FakePlayer::new();
        let session = PlaybackSessionId::new();

        fake.send(PlayerCommand::LoadTemporary {
            display_name: "test.mp3".into(),
            path: std::path::PathBuf::from("/tmp/test.mp3"),
            session_id: session,
        })
        .unwrap();

        assert_eq!(fake.snapshot().state, PlaybackState::Playing);
        assert_eq!(fake.last_session(), Some(session));
    }

    #[test]
    fn play_pause_toggle_respects_state_machine() {
        let fake = FakePlayer::new();
        let s = song();

        // Initially stopped — Play is not a legal transition from Stopped.
        fake.send(PlayerCommand::Play).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Stopped);

        // Load → Playing
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: s,
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);

        // Pause
        fake.send(PlayerCommand::Pause).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Paused);

        // Play from Paused
        fake.send(PlayerCommand::Play).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);

        // TogglePlayPause → Paused
        fake.send(PlayerCommand::TogglePlayPause).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Paused);

        // TogglePlayPause → Playing
        fake.send(PlayerCommand::TogglePlayPause).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);
    }

    #[test]
    fn next_previous_set_ended() {
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();

        fake.send(PlayerCommand::Next).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Ended);
        assert!(fake.snapshot().position.is_none());

        // Load again and try Previous
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        fake.send(PlayerCommand::Previous).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Ended);
    }

    #[test]
    fn seek_updates_position() {
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();

        fake.send(PlayerCommand::Seek(42.5)).unwrap();
        assert_eq!(fake.snapshot().position, Some(42.5));
    }

    #[test]
    fn volume_clamped_and_mute_toggle() {
        let fake = FakePlayer::new();

        fake.send(PlayerCommand::SetVolume(0.75)).unwrap();
        assert!((fake.snapshot().volume - 0.75).abs() < f64::EPSILON);
        assert!(!fake.snapshot().muted);

        // Clamped above 1.0
        fake.send(PlayerCommand::SetVolume(1.5)).unwrap();
        assert!((fake.snapshot().volume - 1.0).abs() < f64::EPSILON);

        // Clamped below 0.0
        fake.send(PlayerCommand::SetVolume(-0.1)).unwrap();
        assert!((fake.snapshot().volume - 0.0).abs() < f64::EPSILON);

        // SetVolume clears mute
        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert!(fake.snapshot().muted);
        fake.send(PlayerCommand::SetVolume(0.5)).unwrap();
        assert!(!fake.snapshot().muted);

        // Toggle mute
        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert!(fake.snapshot().muted);
        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert!(!fake.snapshot().muted);
    }

    #[test]
    fn unmute_restores_last_nonzero_volume() {
        // Task 8.8: muting remembers the current audible volume; unmuting
        // restores it. Mirrors the actor's semantics.
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::SetVolume(0.3)).unwrap();
        assert!((fake.snapshot().volume - 0.3).abs() < f64::EPSILON);

        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert!(fake.snapshot().muted);
        assert!(
            (fake.snapshot().volume - 0.3).abs() < f64::EPSILON,
            "muting keeps the volume field (mute is only the flag, like mpv)"
        );

        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert!(!fake.snapshot().muted);
        assert!(
            (fake.snapshot().volume - 0.3).abs() < f64::EPSILON,
            "unmute restores the recent non-zero volume"
        );
    }

    #[test]
    fn rejected_property_write_leaves_snapshot_unchanged() {
        // Task 8.8 command-failure rollback: a rejected seek / volume / mute
        // must not touch the snapshot — the UI (driven by the snapshot) stays
        // consistent with what the backend actually accepted.
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::SetVolume(0.5)).unwrap();
        let before_state = fake.snapshot();

        fake.fail_next_property();
        fake.send(PlayerCommand::Seek(9.0)).unwrap();
        assert_eq!(fake.snapshot().position, before_state.position);
        assert_eq!(fake.snapshot().volume, before_state.volume);

        fake.fail_next_property();
        fake.send(PlayerCommand::SetVolume(0.9)).unwrap();
        assert_eq!(fake.snapshot().volume, before_state.volume);

        fake.fail_next_property();
        fake.send(PlayerCommand::ToggleMute).unwrap();
        assert_eq!(fake.snapshot().muted, before_state.muted);
    }

    #[test]
    fn stop_clears_current_item() {
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        assert!(fake.snapshot().current_item.is_some());

        fake.send(PlayerCommand::Stop).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Stopped);
        assert!(fake.snapshot().current_item.is_none());
        assert!(fake.snapshot().position.is_none());
    }

    #[test]
    fn shutdown_blocks_further_commands() {
        let fake = FakePlayer::new();
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();

        fake.send(PlayerCommand::Shutdown).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Stopped);

        // Subsequent sends fail
        let result = fake.send(PlayerCommand::Play);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PlayerError::ActorClosed));
    }

    #[test]
    fn subscriber_receives_snapshots() {
        let fake = FakePlayer::new();
        let rx = fake.subscribe_snapshots();

        // Initial snapshot is not pushed — only state changes are published.
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();

        let snapshots = FakePlayer::drain_snapshots(&rx);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, PlaybackState::Playing);
    }

    #[test]
    fn dead_subscriber_pruned_on_publish() {
        let fake = FakePlayer::new();
        let rx = fake.subscribe_snapshots();
        drop(rx); // drop the receiver

        // Publishing should not panic; the dead subscriber is pruned.
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
    }

    #[test]
    fn manual_set_state_and_position() {
        let fake = FakePlayer::new();
        fake.set_state(PlaybackState::Playing);
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);

        fake.set_position(123.4);
        assert_eq!(fake.snapshot().position, Some(123.4));

        fake.set_mode(PlayMode::Shuffle);
        assert_eq!(fake.snapshot().mode, PlayMode::Shuffle);
    }

    #[test]
    fn queue_stats_no_libmpv() {
        // Simulate a coordinator building a 3-item queue.
        let fake = FakePlayer::new();

        // Load first song
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        {
            let mut guard = fake.inner.lock().expect("fake poisoned");
            guard.snapshot.queue_len = 3;
        }

        let snap = fake.snapshot();
        assert_eq!(snap.state, PlaybackState::Playing);
        assert_eq!(snap.queue_len, 3);
        assert!(snap.current_item.is_some());

        // Pause
        fake.send(PlayerCommand::Pause).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Paused);

        // Advance: Next → Ended, coordinator loads next
        fake.send(PlayerCommand::Next).unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Ended);

        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        assert_eq!(fake.snapshot().state, PlaybackState::Playing);
    }

    #[test]
    fn repeat_one_mode_persists() {
        let fake = FakePlayer::new();
        fake.set_mode(PlayMode::RepeatOne);
        assert_eq!(fake.snapshot().mode, PlayMode::RepeatOne);

        // Mode is independent of playback state changes.
        fake.send(PlayerCommand::LoadLibrarySong {
            song_id: song(),
            session_id: PlaybackSessionId::new(),
        })
        .unwrap();
        assert_eq!(fake.snapshot().mode, PlayMode::RepeatOne);
    }
}
