//! Guarded library-root handoff for the LAN sync session.
//!
//! The dialog, candidate preparation and Core activation stay in the desktop
//! service; this module owns only the session side of the same boundary:
//!
//! 1. [`SyncRuntime::begin_root_handoff`] fences the running session **without**
//!    rewriting the persisted intent, so a cancelled or failed activation can
//!    restore exactly what the user had.
//! 2. [`SyncRuntime::complete_root_handoff`] runs after Core's activation
//!    transaction committed. It restores only the target root's own session,
//!    and only when that root's binding and enabled intent allow it.
//! 3. [`SyncRuntime::abort_root_handoff`] runs when activation failed and puts
//!    the previous root's session back.
//!
//! Nothing here performs discovery, listening or connection work before the
//! activation boundary; a cancelled picker never reaches this module at all.

#[allow(clippy::wildcard_imports)]
use super::*;

/// What the session looked like immediately before the handoff fence.
pub struct RootHandoff {
    root: Option<LibraryRootId>,
    was_enabled: bool,
    role: SyncRole,
    address: Option<IpAddr>,
}

impl SyncRuntime {
    /// Fence the running session for a root switch without touching intent.
    ///
    /// This is the only place that stops workers *and* deliberately keeps the
    /// persisted lifecycle choice untouched: a stop here is a handoff step, not
    /// a user stop of this library.
    pub fn begin_root_handoff(&self) -> Result<RootHandoff, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let (previous_root, previous_enabled, previous_role, previous_address) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            (
                inner.root,
                inner.status.enabled,
                inner.status.role,
                inner.host_address,
            )
        };
        self.cancel_restore_retry();
        if previous_enabled {
            self.stop_inner()?;
        }
        Ok(RootHandoff {
            root: previous_root,
            was_enabled: previous_enabled,
            role: previous_role,
            address: previous_address,
        })
    }

    /// Restore the target root's session from its own enabled intent.
    ///
    /// A stopped, paused or unbound target root stays closed. The returned
    /// status always reflects the new root, so the UI never keeps showing the
    /// previous session after a successful activation.
    pub fn complete_root_handoff(
        &self,
        root: LibraryRootId,
    ) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        // Activation already selected the context inside its transaction; this
        // re-select is an idempotent guard for a handoff that skipped it.
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        match self.restore_locked() {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                // No restore: refresh the root-scoped status and intent so a
                // stopped or paused library is still reported correctly.
                let status = self.reset_status_for_root(root);
                self.notify_status(status.clone());
                Ok(status)
            }
            Err(error) => {
                self.notify_status(self.report_error(error));
                Err(error)
            }
        }
    }

    /// Put the previous root's session back after a failed activation.
    ///
    /// The activation transaction is atomic, so the previous root is still
    /// active here; restoring it is exactly the pre-switch restoration path.
    pub fn abort_root_handoff(
        &self,
        handoff: &RootHandoff,
    ) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let Some(previous_root) = handoff.root else {
            return Ok(self.status());
        };
        if self.active_root().ok() != Some(previous_root) {
            // The activation did change the active root; restoring the previous
            // session would act on another library, so refuse instead of
            // guessing which session the user should keep.
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.database
            .select_sync_context(previous_root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if !handoff.was_enabled {
            let status = self.reset_status_for_root(previous_root);
            self.notify_status(status.clone());
            return Ok(status);
        }
        match handoff.role {
            SyncRole::Host => match handoff.address {
                Some(address) => self.start_host_locked(address),
                None => self
                    .restore_locked()?
                    .ok_or(SyncRuntimeError::InvalidAddress),
            },
            _ => self.start_peer_locked(),
        }
        .inspect_err(|error| {
            // A failed restore must still be visible: the library is active but
            // its session is not, which the UI has to distinguish from "stopped".
            self.notify_status(self.report_error(*error));
        })
    }

    /// Reset the typed status to a closed session for one root, reading that
    /// root's persisted intent so `stopped` and `paused` stay distinct.
    fn reset_status_for_root(&self, root: LibraryRootId) -> SyncStatus {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = inner.status.session_id;
        let sequence = inner.status.event_sequence + 1;
        let binding = self
            .database
            .current_sync_configuration()
            .ok()
            .flatten()
            .filter(|configuration| configuration.local_root == root)
            .map(|configuration| configuration.binding);
        inner.status = SyncStatus {
            session_id: session,
            event_sequence: sequence,
            // A closed session still names the library it belongs to, so the
            // UI can distinguish "this new root is stopped" from a stale event
            // belonging to the root it just left.
            library_id: binding.map(|binding| binding.library_id.as_uuid().to_string()),
            local_root: Some(root.as_uuid().to_string()),
            ..SyncStatus::default()
        };
        inner.root = None;
        inner.host_address = None;
        inner.host_certificate_der = None;
        inner.auto_sync = false;
        inner.manual_cycle_cancel = None;
        drop(inner);
        self.status()
    }
}

#[cfg(test)]
#[path = "tests/handoff.rs"]
mod tests;
