//! Opt-in native LAN lifecycle. Provisioning never accepts keys from the `WebView`.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use echo_core::application::ports::{DeviceIdProvider, LibraryRepository};
use echo_core::domain::ids::LibraryRootId;
use echo_core::domain::sync::SyncBinding;
use echo_core::infrastructure::sqlite::SqliteDatabase;
pub use echo_desktop::ipc::sync::{DiscoveredHost, SyncRole, SyncState, SyncStatus};
use echo_sync::discovery::LanDiscovery;
use echo_sync::pairing::{PairingError, PairingGrantDto, PairingInvitation, PendingPairingRequest};
use echo_sync::server::{host_router, serve_tls, HostApiState};
use tokio::sync::oneshot;

pub const SYNC_PORT: u16 = 50826;

mod discovery;
pub use discovery::discover_hosts;

pub type StatusNotifier = Arc<dyn Fn(SyncStatus) + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncRuntimeError {
    Unconfigured,
    InvalidAddress,
    LibraryUnavailable,
    Busy,
    BindFailed,
    DiscoveryFailed,
    PairingFailed,
    PairingRequestRejected,
    PairingRateLimited,
    PairingHostUnavailable,
    PairingConnectionFailed,
    PairingProtocolFailed,
    WorkerFailed,
}

impl SyncRuntimeError {
    pub const fn message_key(self) -> &'static str {
        match self {
            Self::Unconfigured => "sync.unconfigured",
            Self::InvalidAddress => "sync.invalidAddress",
            Self::LibraryUnavailable => "sync.libraryUnavailable",
            Self::Busy => "sync.busy",
            Self::BindFailed => "sync.bindFailed",
            Self::DiscoveryFailed => "sync.discoveryFailed",
            Self::PairingFailed => "sync.pairingCodeInvalid",
            Self::PairingRequestRejected => "sync.pairingRequestRejected",
            Self::PairingRateLimited => "sync.pairingRateLimited",
            Self::PairingHostUnavailable => "sync.pairingHostUnavailable",
            Self::PairingConnectionFailed => "sync.pairingConnectionFailed",
            Self::PairingProtocolFailed => "sync.pairingProtocolFailed",
            Self::WorkerFailed => "sync.workerFailed",
        }
    }
}

/// Returned only by trusted native provisioning after recovering the library,
/// baseline and independent publication checkpoint and loading TLS from the OS
/// secure store. API state must include live authorization and snapshot ports.
pub struct ProvisionedHost {
    pub binding: SyncBinding,
    pub api: HostApiState,
    pub tls: rustls::ServerConfig,
    pub certificate_der: Vec<u8>,
    pub advertisement_name: String,
    pub device_id: String,
}

pub struct ProvisionedPeer {
    pub binding: SyncBinding,
    pub address: String,
    pub client: echo_sync::client::LanClient,
    pub host_device_id: echo_core::domain::sync::DeviceId,
}

/// Native composition owns pairing, secure credentials and Core orchestration.
/// An absent adapter is explicitly unavailable; no memory-only credentials or
/// plaintext fallback are substituted for durable platform provisioning.
pub trait SyncProvisioner: Send + Sync {
    fn host(&self, root: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError>;
    fn peer(&self, root: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError>;
    fn sync_now(&self, role: SyncRole) -> Result<(), SyncRuntimeError>;
    /// Cancel an in-flight background cycle when the user pauses or stops the
    /// session. Provisioners with cancellable transports should override this;
    /// the default preserves compatibility with simple adapters.
    fn sync_now_cancellable(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), SyncRuntimeError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(SyncRuntimeError::Busy);
        }
        self.sync_now(role)
    }
    fn revoke_device(&self, device_id: &str) -> Result<(), SyncRuntimeError>;
    fn complete_peer_pairing(
        &self,
        root: LibraryRootId,
        invitation: &PairingInvitation,
        endpoint: &str,
        grant: &PairingGrantDto,
    ) -> Result<(), SyncRuntimeError>;
}

struct HostWorker {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    _advertisement: LanDiscovery,
}

struct SyncLoopWorker {
    stop: Arc<AtomicBool>,
    wake: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl SyncLoopWorker {
    fn stop(mut self) -> Result<(), SyncRuntimeError> {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        Ok(())
    }
}

impl Drop for SyncLoopWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl HostWorker {
    fn stop(mut self) -> Result<(), SyncRuntimeError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        Ok(())
    }
}

impl Drop for HostWorker {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Inner {
    status: SyncStatus,
    host: Option<HostWorker>,
    sync_loop: Option<SyncLoopWorker>,
    auto_sync: bool,
    manual_cycle_cancel: Option<Arc<AtomicBool>>,
    root: Option<LibraryRootId>,
    host_address: Option<IpAddr>,
    pairing_api: Option<HostApiState>,
    host_certificate_der: Option<Vec<u8>>,
}

pub struct SyncRuntime {
    database: Arc<SqliteDatabase>,
    provisioner: Option<Arc<dyn SyncProvisioner>>,
    // Serialize transitions independently from status. Status never waits for
    // TLS shutdown, discovery, SQLite, or a remote round trip.
    transition: Mutex<()>,
    cycle: Arc<Mutex<()>>,
    status_notifier: Arc<Mutex<Option<StatusNotifier>>>,
    inner: Arc<Mutex<Inner>>,
}

impl SyncRuntime {
    pub fn new(
        database: Arc<SqliteDatabase>,
        provisioner: Option<Arc<dyn SyncProvisioner>>,
    ) -> Self {
        Self {
            database,
            provisioner,
            transition: Mutex::new(()),
            cycle: Arc::new(Mutex::new(())),
            status_notifier: Arc::new(Mutex::new(None)),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn set_status_notifier(&self, notifier: StatusNotifier) {
        *self
            .status_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notifier);
    }

    pub fn status(&self) -> SyncStatus {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner
            .host
            .as_ref()
            .is_some_and(|host| host.thread.as_ref().is_some_and(JoinHandle::is_finished))
            && inner.status.state != SyncState::Failed
        {
            inner.status.state = SyncState::Failed;
            inner.status.last_error = Some(SyncRuntimeError::WorkerFailed.message_key().into());
            inner.status.event_sequence += 1;
        }
        inner.status.clone()
    }

    pub fn host_start(&self, address: &str) -> Result<SyncStatus, SyncRuntimeError> {
        let address = selected_lan_address(address)?;
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.start_host_locked(address)
    }

    fn start_host_locked(&self, address: IpAddr) -> Result<SyncStatus, SyncRuntimeError> {
        if self.status().enabled {
            return Err(SyncRuntimeError::Busy);
        }
        let provisioner = self
            .provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let root = self.active_root()?;
        let host = provisioner.host(root)?;
        if self.active_root()? != root {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let listener = TcpListener::bind(SocketAddr::new(address, SYNC_PORT))
            .map_err(|_| SyncRuntimeError::BindFailed)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| SyncRuntimeError::BindFailed)?;
        let socket = listener
            .local_addr()
            .map_err(|_| SyncRuntimeError::BindFailed)?;
        let advertisement = LanDiscovery::advertise(
            &host.advertisement_name,
            "echo-host.local.",
            address,
            socket.port(),
            &host.device_id,
            echo_sync::protocol::PROTOCOL_VERSION,
        )
        .map_err(|_| SyncRuntimeError::DiscoveryFailed)?;
        let executor = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let (stop, stopped) = oneshot::channel();
        let (ready, readiness) = std::sync::mpsc::sync_channel(1);
        let state = Arc::clone(&self.inner);
        let binding = host.binding;
        let pairing_api = host.api.clone();
        let host_certificate_der = host.certificate_der.clone();
        let worker = thread::Builder::new().name("echo-lan-host".into()).spawn(move || {
            executor.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    let _ = ready.send(false);
                    return;
                };
                let _ = ready.send(true);
                tokio::select! {
                    _ = stopped => {},
                    result = serve_tls(listener, host.tls, host_router(host.api)) => {
                        if result.is_err() {
                            let mut inner = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            inner.status.state = SyncState::Failed;
                            inner.status.last_error = Some(SyncRuntimeError::WorkerFailed.message_key().into());
                            inner.status.event_sequence += 1;
                        }
                    }
                }
            });
            // Dropping the dedicated executor cancels its TLS connections and
            // waits for every in-flight blocking Core transaction/publication.
            // Stop returns only after that durable work has converged.
            drop(executor);
        }).map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let host_worker = HostWorker {
            stop: Some(stop),
            thread: Some(worker),
            _advertisement: advertisement,
        };
        if readiness.recv() != Ok(true) {
            return Err(SyncRuntimeError::WorkerFailed);
        }
        self.set_started(root, binding, SyncRole::Host, format!("https://{socket}"));
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        inner.host = Some(host_worker);
        inner.host_address = Some(address);
        inner.pairing_api = Some(pairing_api);
        inner.host_certificate_der = Some(host_certificate_der);
        drop(inner);
        if let Err(error) = self.start_sync_loop(SyncRole::Host) {
            let _ = self.stop_inner();
            return Err(error);
        }
        Ok(self.status())
    }

    pub fn peer_start(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.start_peer_locked()
    }

    pub fn local_sync_device_id(&self) -> echo_core::domain::sync::DeviceId {
        echo_core::domain::sync::DeviceId::from_uuid(self.database.current_device_id().as_uuid())
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

    fn start_peer_locked(&self) -> Result<SyncStatus, SyncRuntimeError> {
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
        let auto_sync = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .auto_sync;
        if auto_sync {
            self.start_sync_loop(SyncRole::Peer)?;
        }
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
        let provisioner = self
            .provisioner
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
        let result = provisioner.sync_now_cancellable(status.role, Arc::clone(&cancelled));
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
        let loop_result = self.start_sync_loop(status.role);
        if let Err(error) = loop_result {
            self.change_state(SyncState::Failed, Some(error.message_key().into()));
            return Err(error);
        }
        match result {
            Ok(()) => self.change_state(
                if status.role == SyncRole::Host {
                    SyncState::Hosting
                } else {
                    SyncState::Connected
                },
                None,
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

    pub fn pending_pairing_requests(&self) -> Result<Vec<PendingPairingRequest>, SyncRuntimeError> {
        let api = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .pairing_api
            .clone()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        api.pending_pairing_requests().map_err(map_pairing_error)
    }

    pub fn approve_pairing_request(&self, request_id: &str) -> Result<(), SyncRuntimeError> {
        let api = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .pairing_api
            .clone()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        api.approve_pairing_request(request_id)
            .map_err(map_pairing_error)
    }

    pub fn deny_pairing_request(&self, request_id: &str) -> Result<(), SyncRuntimeError> {
        let api = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .pairing_api
            .clone()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        api.deny_pairing_request(request_id)
            .map_err(map_pairing_error)
    }

    fn active_root(&self) -> Result<LibraryRootId, SyncRuntimeError> {
        self.database
            .active_root()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .map(|root| root.id())
            .ok_or(SyncRuntimeError::LibraryUnavailable)
    }

    fn set_started(
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
        inner.status.address = Some(address);
        inner.status.last_error = None;
        inner.root = Some(root);
    }

    fn change_state(&self, state: SyncState, error: Option<String>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.state = state;
        inner.status.last_error = error;
        inner.status.event_sequence += 1;
    }

    #[allow(clippy::too_many_lines)]
    fn start_sync_loop(&self, role: SyncRole) -> Result<(), SyncRuntimeError> {
        let provisioner = self
            .provisioner
            .clone()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if inner.sync_loop.is_some() {
            inner.auto_sync = true;
            return Ok(());
        }
        if !inner.status.enabled || inner.status.role != role {
            return Err(SyncRuntimeError::Unconfigured);
        }
        let session_id = inner.status.session_id;
        let state = Arc::clone(&self.inner);
        let notifier = Arc::clone(&self.status_notifier);
        let cycle = Arc::clone(&self.cycle);
        let (wake, wakeups) = mpsc::channel();
        self.database.register_sync_waker(wake.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("echo-lan-sync-loop".into())
            .spawn(move || {
                let poll_interval = Duration::from_secs(5);
                let mut retry_delay = Duration::from_secs(5);
                let mut retry_attempts = 0_u32;
                let mut retrying = false;
                loop {
                    let delay = if retrying {
                        jittered_retry_delay(retry_delay)
                    } else {
                        poll_interval
                    };
                    if !wait_for_sync_wakeup(&wakeups, delay, retrying, &worker_stop) {
                        break;
                    }
                    let should_run = {
                        let mut inner = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if inner.status.session_id != session_id
                            || !inner.status.enabled
                            || inner.status.state == SyncState::Paused
                        {
                            false
                        } else {
                            inner.status.state = SyncState::Syncing;
                            inner.status.last_error = None;
                            inner.status.event_sequence += 1;
                            true
                        }
                    };
                    if !should_run {
                        break;
                    }
                    let status_notifier = notifier
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(notifier) = status_notifier {
                        let status = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .status
                            .clone();
                        notifier(status);
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let result = {
                        let Ok(_cycle) = cycle.lock() else {
                            break;
                        };
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        provisioner.sync_now_cancellable(role, Arc::clone(&worker_stop))
                    };
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    {
                        let mut inner = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if inner.status.session_id != session_id || !inner.status.enabled {
                            break;
                        }
                        inner.status.event_sequence += 1;
                        match result {
                            Ok(()) => {
                                inner.status.state = if role == SyncRole::Host {
                                    SyncState::Hosting
                                } else {
                                    SyncState::Connected
                                };
                                inner.status.last_error = None;
                                retrying = false;
                                retry_delay = poll_interval;
                                retry_attempts = 0;
                            }
                            Err(error) => {
                                inner.status.state = SyncState::Failed;
                                inner.status.last_error = Some(error.message_key().into());
                                retrying = true;
                                if retry_attempts > 0 {
                                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                                }
                                retry_attempts = retry_attempts.saturating_add(1);
                            }
                        }
                        drop(inner);
                    }
                    let status_notifier = notifier
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(notifier) = status_notifier {
                        let status = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .status
                            .clone();
                        notifier(status);
                    }
                }
            })
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        inner.auto_sync = true;
        inner.sync_loop = Some(SyncLoopWorker {
            stop,
            wake,
            thread: Some(thread),
        });
        drop(inner);
        Ok(())
    }

    fn stop_sync_loop(&self) -> Result<(), SyncRuntimeError> {
        let worker = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .sync_loop
            .take();
        if let Some(worker) = worker {
            worker.stop()?;
        }
        Ok(())
    }

    fn stop_inner(&self) -> Result<(), SyncRuntimeError> {
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
        let (host, pairing_api) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            (inner.host.take(), inner.pairing_api.take())
        };
        let mut pairing_result = Ok(());
        if let Some(api) = pairing_api {
            if api.cancel_pairing_requests().is_err() {
                pairing_result = Err(SyncRuntimeError::LibraryUnavailable);
            }
        }
        if let Some(host) = host {
            host.stop()?;
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let sequence = inner.status.event_sequence + 1;
        let session = inner.status.session_id;
        inner.status = SyncStatus {
            session_id: session,
            event_sequence: sequence,
            ..SyncStatus::default()
        };
        inner.root = None;
        inner.host_address = None;
        inner.host_certificate_der = None;
        inner.auto_sync = false;
        inner.manual_cycle_cancel = None;
        drop(inner);
        pairing_result
    }
}

fn jittered_retry_delay(base: Duration) -> Duration {
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

fn wait_for_sync_wakeup(
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

const fn map_pairing_error(error: PairingError) -> SyncRuntimeError {
    match error {
        PairingError::Capacity | PairingError::Pending => SyncRuntimeError::Busy,
        PairingError::InvalidRequest => SyncRuntimeError::InvalidAddress,
        PairingError::Unavailable | PairingError::ProvisioningUnavailable => {
            SyncRuntimeError::Unconfigured
        }
    }
}

impl Drop for SyncRuntime {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

pub fn selected_lan_address(address: &str) -> Result<IpAddr, SyncRuntimeError> {
    let address: IpAddr = address
        .parse()
        .map_err(|_| SyncRuntimeError::InvalidAddress)?;
    let valid = match address {
        IpAddr::V4(address) => address.is_private() || address.is_link_local(),
        IpAddr::V6(address) => (address.segments()[0] & 0xfe00) == 0xfc00,
    };
    if valid {
        Ok(address)
    } else {
        Err(SyncRuntimeError::InvalidAddress)
    }
}

#[cfg(test)]
#[path = "sync_runtime/tests.rs"]
mod tests;
