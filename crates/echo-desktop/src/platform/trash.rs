//! Production system-trash boundary (task 9.5).
//!
//! The OS call that moves `trash/<operation-id>` into the recycle bin is
//! injected as a [`TrashBackend`] — the composition root wires the real
//! `trash`-crate call per platform. This module owns the platform policies
//! around that call that are proven without an OS:
//!
//!   - **Windows file-lock retry**: a locked file is transient (another app
//!     holds it open); the call is retried a bounded number of times with a
//!     short backoff before a failure is surfaced.
//!   - **The irreversibility point**: `send_to_trash` returns `Ok(())` ONLY on
//!     an unambiguous backend success. A lock that never clears, a vanished
//!     staging dir, or any indeterminate outcome becomes an error, so the
//!     journal keeps the operation pending (`TrashPending`/unknown) — a
//!     fabricated success would silently drop a delete.
//!
//! Every entry takes [`LibraryRootId`] + [`OperationId`] — never a raw path —
//! so the caller cannot reach arbitrary filesystem targets through it.

use std::time::Duration;

use echo_core::application::ports::SystemTrashPort;
use echo_core::domain::ids::{LibraryRootId, OperationId};
use echo_core::error::Error;

/// Maximum number of attempts for a trash call that fails with a file lock
/// (Windows). Bounded so a permanently-locked file cannot spin forever.
pub const LOCK_RETRY_ATTEMPTS: u32 = 3;
/// Backoff inserted between lock-retry attempts.
pub const LOCK_RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// Why a trash backend call failed — the retry policy only retries a
/// [`TrashBackendError::Locked`] outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrashBackendError {
    /// The target is locked (Windows `/Windows` file lock) or otherwise
    /// transiently busy; retrying may help.
    Locked,
    /// Any other failure — permanent or indeterminate; do not retry as a lock.
    Other,
}

impl std::fmt::Display for TrashBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => write!(f, "staging directory is locked"),
            Self::Other => write!(f, "system trash call failed"),
        }
    }
}

impl std::error::Error for TrashBackendError {}

/// The injected OS call: move the staging directory for `operation` of `root`
/// into the system recycle bin. It returns `Err(Locked)` when the platform
/// reports a transient file lock and `Err(Other)` for every other failure.
pub trait TrashBackend: Send + Sync {
    /// # Errors
    ///
    /// [`TrashBackendError::Locked`] for a transient lock; [`TrashBackendError::Other`]
    /// for any other failure. The caller must treat an `Err` as "not trashed" —
    /// never as a success.
    fn move_to_trash(
        &self,
        root: &LibraryRootId,
        operation: &OperationId,
    ) -> Result<(), TrashBackendError>;
}

/// A [`SystemTrashPort`] over an injected [`TrashBackend`], applying the
/// Windows file-lock retry policy and the irreversibility contract.
#[derive(Clone, Copy, Debug)]
pub struct DesktopTrash {
    /// The backend to retry. Retried in place (the caller supplies a concrete
    /// backend type).
    attempts: u32,
    backoff: Duration,
}

impl DesktopTrash {
    /// The production retry budget: [`LOCK_RETRY_ATTEMPTS`] tries,
    /// [`LOCK_RETRY_BACKOFF`] apart.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            attempts: LOCK_RETRY_ATTEMPTS,
            backoff: LOCK_RETRY_BACKOFF,
        }
    }

    /// Run one `send_to_trash` against `backend`, retrying lock failures up to
    /// the configured budget. `Ok` is returned only when `backend` succeeded
    /// unambiguously; any other terminal outcome is an error. `root`/`operation`
    /// are the resolved ids — never raw paths.
    ///
    /// # Errors
    ///
    /// `Locked`: the file remained locked past the retry budget.
    /// `Unavailable`/`Storage`-ish mapping: the backend reported a permanent or
    /// indeterminate failure (mapped to a [`Error`] the journal preserves).
    pub fn send_to_trash(
        &self,
        backend: &dyn TrashBackend,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error> {
        for attempt in 0..self.attempts {
            match backend.move_to_trash(&root, &operation) {
                Ok(()) => return Ok(()),
                Err(TrashBackendError::Locked) if attempt + 1 < self.attempts => {
                    std::thread::sleep(self.backoff);
                }
                Err(TrashBackendError::Locked) => {
                    return Err(Error::unavailable(
                        "system trash",
                        "file remained locked after retries (Windows)",
                    ));
                }
                Err(TrashBackendError::Other) => {
                    return Err(Error::unavailable(
                        "system trash",
                        "system trash call failed / outcome indeterminate",
                    ));
                }
            }
        }
        Err(Error::unavailable(
            "system trash",
            "no trash attempt was made",
        ))
    }
}

impl Default for DesktopTrash {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTrashPort for DesktopTrash {
    fn send_to_trash(&self, _root: LibraryRootId, _operation: OperationId) -> Result<(), Error> {
        // `DesktopTrash` holds no backend; the real composition root uses a
        // backend-carrying wrapper. This default never has a backend and thus
        // cannot succeed — it exists so the port shape is concrete and tested
        // without injecting an OS call.
        Err(Error::unavailable(
            "system trash",
            "no trash backend connected yet",
        ))
    }
}

/// A [`SystemTrashPort`] that carries a concrete backend. `D` is the backend
/// type the composition root provides.
#[derive(Clone, Copy, Debug)]
pub struct TrashWithBackend<D> {
    backend: D,
    policy: DesktopTrash,
}

impl<D: TrashBackend> TrashWithBackend<D> {
    #[must_use]
    pub const fn new(backend: D) -> Self {
        Self {
            backend,
            policy: DesktopTrash::new(),
        }
    }
}

impl<D: TrashBackend> SystemTrashPort for TrashWithBackend<D> {
    fn send_to_trash(&self, root: LibraryRootId, operation: OperationId) -> Result<(), Error> {
        self.policy.send_to_trash(&self.backend, root, operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Clone, Copy, Debug)]
    enum Behavior {
        Succeeds,
        LockedOnce,
        LockedAlways,
        Other,
    }

    #[derive(Debug)]
    struct Stub {
        behavior: Behavior,
        calls: std::sync::Arc<AtomicU32>,
    }

    impl Stub {
        fn new(behavior: Behavior) -> (Self, std::sync::Arc<AtomicU32>) {
            let calls = std::sync::Arc::new(AtomicU32::new(0));
            (
                Self {
                    behavior,
                    calls: std::sync::Arc::clone(&calls),
                },
                calls,
            )
        }
    }

    impl TrashBackend for Stub {
        fn move_to_trash(
            &self,
            _root: &LibraryRootId,
            _operation: &OperationId,
        ) -> Result<(), TrashBackendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.behavior {
                Behavior::Succeeds => Ok(()),
                Behavior::LockedOnce => {
                    if self.calls.load(Ordering::SeqCst) == 1 {
                        Err(TrashBackendError::Locked)
                    } else {
                        Ok(())
                    }
                }
                Behavior::LockedAlways => Err(TrashBackendError::Locked),
                Behavior::Other => Err(TrashBackendError::Other),
            }
        }
    }

    fn ids() -> (LibraryRootId, OperationId) {
        use std::str::FromStr;
        let root =
            LibraryRootId::from_str("00000000-0000-0000-0000-000000000001").expect("valid root id");
        let op = OperationId::from_str("00000000-0000-0000-0000-000000000002")
            .expect("valid operation id");
        (root, op)
    }

    #[test]
    fn success_is_unambiguous() {
        let (backend, calls) = Stub::new(Behavior::Succeeds);
        let (root, op) = ids();
        let trash = TrashWithBackend::new(backend);
        trash.send_to_trash(root, op).expect("clean trash");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no unnecessary retries");
    }

    #[test]
    fn a_lock_that_clears_is_retried_and_succeeds() {
        let (backend, calls) = Stub::new(Behavior::LockedOnce);
        let (root, op) = ids();
        let trash = TrashWithBackend::new(backend);
        trash
            .send_to_trash(root, op)
            .expect("retry clears the lock");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "locked once, retried once");
    }

    #[test]
    fn a_lock_that_never_clears_is_bounded_not_spun() {
        let (backend, calls) = Stub::new(Behavior::LockedAlways);
        let (root, op) = ids();
        let trash = TrashWithBackend::new(backend);
        // Bounded retries, then a surfaced error — never a fabricated success.
        assert!(trash.send_to_trash(root, op).is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            LOCK_RETRY_ATTEMPTS,
            "retried only the budget, then failed"
        );
    }

    #[test]
    fn a_permanent_failure_is_not_retried_as_a_lock() {
        let (backend, calls) = Stub::new(Behavior::Other);
        let (root, op) = ids();
        let trash = TrashWithBackend::new(backend);
        assert!(trash.send_to_trash(root, op).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "other errors are terminal");
    }

    #[test]
    fn default_desktop_trash_without_backend_never_fabricates_success() {
        let (root, op) = ids();
        let trash = DesktopTrash::new();
        // Call through the port trait (the inherent policy method needs a
        // backend); with none connected, the port never invents a success.
        let port: &dyn SystemTrashPort = &trash;
        assert!(port.send_to_trash(root, op).is_err());
    }
}
