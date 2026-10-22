//! Bounded, cancellable startup restoration of a saved LAN sync session.
//!
//! A previous session may be restored only while the selected library still has
//! an enabled intent and a valid binding. Automatic restoration is therefore
//! finite: every attempt is fenced by a generation token that any user
//! transition (stop, pause, leave, root switch), successful start, or explicit
//! retry advances. Waits are interruptible so a user action never has to wait
//! out a backoff, and exhaustion becomes a visible terminal state with an
//! explicit retry path rather than a silent background loop.

#![allow(clippy::redundant_pub_crate)] // module-private helper split from `main`/`sync_runtime`.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Bounded attempt count for one automatic restoration. A temporarily
/// unavailable host still leaves the user an explicit retry.
pub(super) const MAX_ATTEMPTS: u32 = 5;
pub(super) const BASE_DELAY: Duration = Duration::from_secs(5);
pub(super) const MAX_DELAY: Duration = Duration::from_secs(60);

/// Interruptible backoff for one restore attempt.
pub(super) struct RestoreRetryWake {
    sender: Option<Sender<()>>,
}

impl RestoreRetryWake {
    pub(super) fn new() -> (Self, Receiver<()>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    /// Wake (and end) a waiting worker. Idempotent: the sender is consumed.
    pub(super) fn cancel(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for RestoreRetryWake {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Wait out one backoff. Returns `true` when the wait was interrupted, which
/// means the worker must stop without another provision attempt.
pub(super) fn wait_for_restore_retry(wakeups: &Receiver<()>, delay: Duration) -> bool {
    wakeups.recv_timeout(delay).is_ok()
}

pub(super) fn next_delay(delay: Duration) -> Duration {
    delay.saturating_mul(2).min(MAX_DELAY)
}

/// Whether a retry still belongs to the current library and user intent.
///
/// Every input is deliberate: a different generation means a later lifecycle
/// transition superseded this worker, a different active root means the user
/// switched libraries, and a disabled intent means the user stopped or paused.
pub(super) const fn retry_allowed(
    observed_generation: u64,
    current_generation: u64,
    active_root_matches: bool,
    intent_enabled: bool,
) -> bool {
    observed_generation == current_generation && active_root_matches && intent_enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(next_delay(BASE_DELAY), Duration::from_secs(10));
        assert_eq!(next_delay(Duration::from_secs(60)), MAX_DELAY);
        assert_eq!(next_delay(MAX_DELAY), MAX_DELAY);
    }

    #[test]
    fn retry_is_fenced_by_generation_root_and_enabled_intent() {
        assert!(retry_allowed(4, 4, true, true));
        assert!(!retry_allowed(4, 5, true, true));
        assert!(!retry_allowed(4, 4, false, true));
        assert!(!retry_allowed(4, 4, true, false));
    }

    #[test]
    fn retry_wait_can_be_woken_before_the_deadline() {
        let (mut wake, wakeups) = RestoreRetryWake::new();
        wake.cancel();
        assert!(wait_for_restore_retry(&wakeups, Duration::from_secs(30)));
    }

    #[test]
    fn dropping_the_wake_ends_a_waiting_worker() {
        let (wake, wakeups) = RestoreRetryWake::new();
        drop(wake);
        assert!(
            wait_for_restore_retry(&wakeups, Duration::from_secs(30)),
            "a dropped owner must not leave the worker sleeping"
        );
    }
}
