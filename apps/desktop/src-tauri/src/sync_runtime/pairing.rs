//! Leaving sync, host recovery, peer authorization and pairing completion.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SyncRuntime {
    pub fn peer_start(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self.running_transition()?;
        self.start_peer_locked()
    }

    /// Restore the persisted LAN role after application startup. Peer sessions
    /// start their normal retrying sync loop, so an unavailable host is retried
    /// without blocking the application. Host mode binds the best available
    /// private LAN address.
    ///
    /// Only a valid binding, a matching active root and an `enabled` intent may
    /// start a session: the binding alone never grants permission to listen or
    /// connect again.
    pub fn restore_saved_session(&self) -> Result<Option<SyncStatus>, SyncRuntimeError> {
        let _transition = self.running_transition()?;
        self.restore_locked()
    }

    /// Restoration that the caller already serialized against lifecycle
    /// transitions. Used by the root handoff after activation committed and by
    /// the automatic retry worker, so neither re-acquires the transition lock.
    pub(super) fn restore_locked(&self) -> Result<Option<SyncStatus>, SyncRuntimeError> {
        self.ensure_running()?;
        let Some(configuration) = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        else {
            return Ok(None);
        };
        if configuration.session_intent != SyncSessionIntent::Enabled
            || self.active_root()? != configuration.local_root
        {
            return Ok(None);
        }
        if self.status().enabled {
            // Already running (for example a concurrent successful start).
            return Ok(Some(self.status()));
        }

        if configuration.host {
            let mut last_error = SyncRuntimeError::DiscoveryFailed;
            for address in available_local_lan_addresses() {
                match self.start_host_locked(address) {
                    Ok(status) => return Ok(Some(status)),
                    Err(error) => last_error = error,
                }
            }
            Err(last_error)
        } else {
            self.start_peer_locked().map(Some)
        }
    }

    /// Stop the current LAN session and remove this library's pairing while
    /// retaining its local catalog and media.
    pub fn leave_sync(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self.running_transition()?;
        let root = self.active_root()?;
        self.cancel_restore_retry();
        self.persist_session_intent(SyncSessionIntent::Stopped)?;
        if self.status().enabled {
            self.stop_inner()?;
        }
        self.provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?
            .leave_sync(root)?;
        Ok(self.status())
    }

    /// Explicitly fence a restored host backup by creating a new authority and
    /// epoch. The host listener is stopped before any durable recovery writes.
    pub fn recover_restored_host(
        &self,
    ) -> Result<echo_core::application::ports::HostRecoveryArchive, SyncRuntimeError> {
        let _transition = self.running_transition()?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .filter(|configuration| configuration.host)
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let root = configuration.local_root;
        if self.status().enabled {
            if self.status().role != SyncRole::Host {
                return Err(SyncRuntimeError::Busy);
            }
            if self.active_root()? != root {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            self.stop_inner()?;
        }
        self.provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?
            .recover_restored_host(root)
    }
}
