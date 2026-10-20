//! Pause/resume/stop, error reporting, immediate sync, device revocation, pairing invitations.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SyncRuntime {
    pub fn local_sync_device_id(&self) -> echo_core::domain::sync::DeviceId {
        echo_core::domain::sync::DeviceId::from_uuid(self.database.current_device_id().as_uuid())
    }

    pub fn authorized_sync_peers(&self) -> Result<Vec<SyncPeer>, SyncRuntimeError> {
        if self.status().role != SyncRole::Host {
            return Err(SyncRuntimeError::Unconfigured);
        }
        self.database
            .authorized_sync_peers()
            .map(|peers| {
                peers
                    .into_iter()
                    .map(|(device_id, display_name)| SyncPeer {
                        device_id: device_id.as_uuid().to_string(),
                        display_name,
                    })
                    .collect()
            })
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
    }

    pub fn complete_peer_pairing(
        &self,
        invitation: &PairingInvitation,
        endpoint: &str,
        grant: &PairingGrantDto,
    ) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if self.status().enabled {
            return Err(SyncRuntimeError::Busy);
        }
        let provisioner = self
            .provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let root = self.active_root()?;
        if Some(root) != self.active_root().ok() {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        provisioner.complete_peer_pairing(root, invitation, endpoint, grant)?;
        self.start_peer_locked()
    }

    pub(super) fn start_peer_locked(&self) -> Result<SyncStatus, SyncRuntimeError> {
        if self.status().enabled {
            return Err(SyncRuntimeError::Busy);
        }
        let provisioner = self
            .provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let root = self.active_root()?;
        let peer = provisioner.peer(root)?;
        if self.active_root()? != root {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.set_started(root, peer.binding, SyncRole::Peer, peer.address);
        // A successful pairing must begin synchronization immediately. The
        // previous auto_sync gate defaulted to false for a fresh runtime, so a
        // newly paired peer could remain "connected" without ever syncing.
        self.start_sync_loop(SyncRole::Peer)?;
        Ok(self.status())
    }

    pub fn stop(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.stop_inner()?;
        Ok(self.status())
    }

    pub fn pause(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if !self.status().enabled {
            return Err(SyncRuntimeError::Unconfigured);
        }
        if let Some(cancelled) = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .manual_cycle_cancel
            .as_ref()
        {
            cancelled.store(true, Ordering::Release);
        }
        self.stop_sync_loop()?;
        let _cycle = self
            .cycle
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let host = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .host
            .take();
        if let Some(host) = host {
            host.stop()?;
        }
        self.change_state(SyncState::Paused, None);
        Ok(self.status())
    }

    pub fn resume(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let status = self.status();
        if status.state != SyncState::Paused {
            return Err(SyncRuntimeError::Busy);
        }
        let (root, address, role, resume_sync) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            (inner.root, inner.host_address, status.role, inner.auto_sync)
        };
        if Some(self.active_root()?) != root {
            self.stop_inner()?;
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.stop_inner()?;
        let result = if role == SyncRole::Host {
            self.start_host_locked(address.ok_or(SyncRuntimeError::InvalidAddress)?)
        } else {
            self.start_peer_locked()
        };
        if resume_sync && role == SyncRole::Peer {
            self.start_sync_loop(SyncRole::Peer)?;
        }
        result
    }

    pub fn report_error(&self, error: SyncRuntimeError) -> SyncStatus {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.status.enabled {
            inner.status.state = SyncState::Failed;
        }
        inner.status.catalog_changed = false;
        inner.status.last_error = Some(error.message_key().into());
        inner.status.event_sequence += 1;
        inner.status.clone()
    }

    pub fn sync_now(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let transition_guard = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let status = self.status();
        if !status.enabled || status.state == SyncState::Paused {
            return Err(SyncRuntimeError::Unconfigured);
        }
        let inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if inner.manual_cycle_cancel.is_some() {
            return Err(SyncRuntimeError::Busy);
        }
        let root = inner.root;
        let session_id = inner.status.session_id;
        drop(inner);
        if Some(self.active_root()?) != root {
            self.stop_inner()?;
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let cycle_port = self
            .cycle_port
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if inner.status.session_id != session_id
            || !inner.status.enabled
            || inner.manual_cycle_cancel.is_some()
        {
            return Err(SyncRuntimeError::Busy);
        }
        inner.manual_cycle_cancel = Some(Arc::clone(&cancelled));
        drop(inner);
        self.change_state(SyncState::Syncing, None);
        drop(transition_guard);
        let cycle = Arc::clone(&self.cycle);
        let cycle_guard = cycle.lock().map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let (result, progress) = run_observed_cycle(
            cycle_port.as_ref(),
            status.role,
            Arc::clone(&cancelled),
            session_id,
            Arc::clone(&self.inner),
            Arc::clone(&self.status_notifier),
        );
        drop(cycle_guard);
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if inner
            .manual_cycle_cancel
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &cancelled))
        {
            inner.manual_cycle_cancel = None;
        }
        let still_active = inner.status.session_id == session_id
            && inner.status.enabled
            && inner.status.state != SyncState::Paused;
        drop(inner);
        if !still_active {
            return Err(SyncRuntimeError::Busy);
        }
        progress.apply(
            &mut self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?
                .status,
        );
        if let Err(error) = self.start_sync_loop(status.role) {
            self.change_state(SyncState::Failed, Some(error.message_key().into()));
            return Err(error);
        }
        match result {
            Ok(catalog_changed) => self.complete_cycle(
                if status.role == SyncRole::Host {
                    SyncState::Hosting
                } else {
                    SyncState::Connected
                },
                catalog_changed,
            ),
            Err(error) => {
                self.change_state(SyncState::Failed, Some(error.message_key().into()));
                return Err(error);
            }
        }
        Ok(self.status())
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<(), SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if self.status().role != SyncRole::Host {
            return Err(SyncRuntimeError::Unconfigured);
        }
        self.provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?
            .revoke_device(device_id)
    }

    pub fn create_pairing_invitation(&self) -> Result<PairingInvitation, SyncRuntimeError> {
        let (api, certificate) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            (
                inner
                    .pairing_api
                    .clone()
                    .ok_or(SyncRuntimeError::Unconfigured)?,
                inner
                    .host_certificate_der
                    .clone()
                    .ok_or(SyncRuntimeError::Unconfigured)?,
            )
        };
        let host_device = echo_core::domain::sync::DeviceId::from_uuid(
            self.database.current_device_id().as_uuid(),
        );
        api.create_pairing_invitation(host_device, certificate, Duration::from_secs(600))
            .map_err(map_pairing_error)
    }
}

impl SyncRuntime {
    pub(super) fn active_root(&self) -> Result<LibraryRootId, SyncRuntimeError> {
        self.database
            .active_root()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .map(|root| root.id())
            .ok_or(SyncRuntimeError::LibraryUnavailable)
    }

    pub(super) fn set_started(
        &self,
        root: LibraryRootId,
        binding: SyncBinding,
        role: SyncRole,
        address: String,
    ) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.session_id += 1;
        inner.status.event_sequence += 1;
        inner.status.enabled = true;
        inner.status.role = role;
        inner.status.state = if role == SyncRole::Host {
            SyncState::Hosting
        } else {
            SyncState::Connected
        };
        inner.status.library_id = Some(binding.library_id.as_uuid().to_string());
        inner.status.local_root = Some(root.as_uuid().to_string());
        inner.status.address = Some(address);
        inner.status.last_error = None;
        inner.status.catalog_changed = false;
        SyncProgress::default().apply(&mut inner.status);
        inner.root = Some(root);
    }

    pub(super) fn change_state(&self, state: SyncState, error: Option<String>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.state = state;
        inner.status.last_error = error;
        inner.status.catalog_changed = false;
        inner.status.event_sequence += 1;
    }

    pub(super) fn complete_cycle(&self, state: SyncState, catalog_changed: bool) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.state = state;
        inner.status.last_error = None;
        inner.status.catalog_changed = catalog_changed;
        inner.status.event_sequence += 1;
    }
}

pub(super) fn jittered_retry_delay(base: Duration) -> Duration {
    use std::time::{SystemTime, UNIX_EPOCH};
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        % 400_001;
    let factor_per_million = 800_000 + u64::from(jitter);
    Duration::from_nanos(
        u64::try_from(
            (base.as_nanos() * u128::from(factor_per_million) / 1_000_000)
                .min(u128::from(u64::MAX)),
        )
        .unwrap_or(u64::MAX),
    )
}

pub(super) fn wait_for_sync_wakeup(
    wakeups: &mpsc::Receiver<()>,
    delay: Duration,
    suppress_early_retry: bool,
    stop: &AtomicBool,
) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return true;
        };
        match wakeups.recv_timeout(remaining) {
            Ok(()) => {
                while wakeups.try_recv().is_ok() {}
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                if !suppress_early_retry {
                    return true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return true,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
}

pub(super) const fn map_pairing_error(error: PairingError) -> SyncRuntimeError {
    match error {
        PairingError::Capacity | PairingError::Pending => SyncRuntimeError::Busy,
        PairingError::InvalidRequest => SyncRuntimeError::InvalidAddress,
        PairingError::Unavailable | PairingError::ProvisioningUnavailable => {
            SyncRuntimeError::Unconfigured
        }
    }
}
