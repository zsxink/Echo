//! Auto-advance watcher (task 8.x): turns natural-EOF / load-failure
//! snapshots into coordinator advances.

use std::sync::{Arc, Mutex};

use super::PlaybackCoordinator;
use super::PlaybackState;
use super::PlayerPort;

/// Spawn the auto-advance watcher: the missing half of the playback loop.
///
/// The coordinator already owns the "what plays next" rules
/// ([`PlaybackCoordinator::on_played_to_end`] / `on_load_error`), but nothing
/// in production ever called them — a track reaching natural EOF published an
/// `ended` snapshot to the UI and the queue stalled there until the user
/// pressed next manually. This thread subscribes to the actor's snapshot
/// stream (subscribers fan out, so the forwarder is unaffected) and drives the
/// coordinator on the transitions the queue rules expect:
///
/// - `Stopped/… → Ended` (natural EOF, or a load failure surfaced as `Ended`):
///   advance per the active mode (sequential/shuffle/repeat-one).
/// - `… → Failed` (unknown song, unresolvable file): error-skip past the
///   entry, never retrying it within the round.
///
/// The transition guard (only firing when the *previous* snapshot was in a
/// different state) keeps a duplicate `Ended` publish from double-advancing.
#[allow(clippy::needless_pass_by_value)] // The port is moved into the worker thread.
pub fn spawn_auto_advance(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-auto-advance".into())
        .spawn(move || {
            let mut last = PlaybackState::Stopped;
            while let Ok(snap) = rx.recv() {
                let state = snap.state;
                if state != last {
                    match state {
                        PlaybackState::Ended => {
                            if let Ok(mut coord) = coordinator.lock() {
                                coord.on_played_to_end();
                            }
                        }
                        PlaybackState::Failed => {
                            if let Ok(mut coord) = coordinator.lock() {
                                coord.on_load_error();
                            }
                        }
                        _ => {}
                    }
                }
                last = state;
            }
        })
        .expect("spawn auto-advance thread");
}
