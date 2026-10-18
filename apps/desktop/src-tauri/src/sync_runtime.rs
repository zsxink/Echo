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
pub use echo_desktop::ipc::sync::{DiscoveredHost, SyncPeer, SyncRole, SyncState, SyncStatus};
use echo_sync::discovery::LanDiscovery;
use echo_sync::pairing::{PairingError, PairingGrantDto, PairingInvitation};
use echo_sync::server::{host_router, serve_tls, HostApiState};
use tokio::sync::oneshot;

pub const SYNC_PORT: u16 = 50826;

/// How long a healthy peer holds one long-poll against the host between
/// cycles. The host answers as soon as it publishes, so this budget is only a
/// safety cadence: each expiry is followed by a full catch-up cycle, and a
/// local write ends the wait immediately.
const PEER_WATCH_BUDGET: Duration = Duration::from_secs(25);

/// Idle cadence for a host that cannot hold a commit page open, and the wait
/// between cycles for the host role. A steady cycle now costs one pull plus one
/// watch request, so 10 s keeps discovery within about 12 requests per minute
/// while a fallback host's changes still surface in seconds.
const FALLBACK_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// First backoff step after a failed cycle; doubles up to a minute.
const RETRY_BASE_DELAY: Duration = Duration::from_secs(5);

mod discovery;
pub use discovery::discover_hosts;

pub type StatusNotifier = Arc<dyn Fn(SyncStatus) + Send + Sync>;
/// The snapshot sequence starts at one and increases within a single cycle.
pub type ProgressNotifier = Arc<dyn Fn(u64, SyncProgress) + Send + Sync>;

/// Only values proven by the native queue and transfer checkpoints are present.
#[derive(Clone, Debug, Default)]
pub struct SyncProgress {
    pub pending_changes: Option<u64>,
    pub pending_uploads: Option<u64>,
    pub pending_downloads: Option<u64>,
    pub uploaded_bytes: Option<u64>,
    pub total_upload_bytes: Option<u64>,
    pub downloaded_bytes: Option<u64>,
    pub total_download_bytes: Option<u64>,
    pub media_ready: Option<bool>,
}

impl SyncProgress {
    const fn apply(self, status: &mut SyncStatus) {
        status.pending_changes = self.pending_changes;
        status.pending_uploads = self.pending_uploads;
        status.pending_downloads = self.pending_downloads;
        status.uploaded_bytes = self.uploaded_bytes;
        status.total_upload_bytes = self.total_upload_bytes;
        status.downloaded_bytes = self.downloaded_bytes;
        status.total_download_bytes = self.total_download_bytes;
        status.media_ready = self.media_ready;
    }
}

/// Why a peer's wait for host changes ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchOutcome {
    /// The host published at least one new commit.
    Advanced,
    /// A local write queued work for the next cycle.
    LocalWrite,
    /// The wait budget elapsed without remote or local work.
    BudgetElapsed,
    /// This provisioner or host cannot watch; the caller keeps plain polling.
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncRuntimeError {
    Unconfigured,
    InvalidAddress,
    LibraryUnavailable,
    LibraryConflict,
    /// Host commits could not be applied to the local catalog.
    SyncRemoteApplyFailed,
    /// The local sync ledger itself failed to read or write.
    SyncLocalStateFailed,
    Busy,
    SyncPredecessorPending,
    ResourcePublicationPending,
    BindFailed,
    DiscoveryFailed,
    PairingFailed,
    PairingRequestRejected,
    PairingRateLimited,
    PairingHostUnavailable,
    PairingConnectionFailed,
    PairingProtocolFailed,
    SyncConnectionFailed,
    SyncHostUnavailable,
    SyncAuthorizationFailed,
    SyncHostRejected,
    SyncBindingMismatch,
    SyncResourceMismatch,
    SyncResourceUnavailable,
    SyncMutationRejected,
    SyncRateLimited,
    SyncCheckpointMismatch,
    SyncProtocolFailed,
    WorkerFailed,
}

impl SyncRuntimeError {
    pub const fn message_key(self) -> &'static str {
        match self {
            Self::Unconfigured => "sync.unconfigured",
            Self::InvalidAddress => "sync.invalidAddress",
            Self::LibraryUnavailable => "sync.libraryUnavailable",
            Self::LibraryConflict => "sync.libraryConflict",
            Self::SyncRemoteApplyFailed => "sync.remoteApplyFailed",
            Self::SyncLocalStateFailed => "sync.localStateFailed",
            Self::Busy => "sync.busy",
            Self::SyncPredecessorPending => "sync.predecessorPending",
            Self::ResourcePublicationPending => "sync.resourcePublicationPending",
            Self::BindFailed => "sync.bindFailed",
            Self::DiscoveryFailed => "sync.discoveryFailed",
            Self::PairingFailed => "sync.pairingCodeInvalid",
            Self::PairingRequestRejected => "sync.pairingRequestRejected",
            Self::PairingRateLimited => "sync.pairingRateLimited",
            Self::PairingHostUnavailable => "sync.pairingHostUnavailable",
            Self::PairingConnectionFailed => "sync.pairingConnectionFailed",
            Self::PairingProtocolFailed => "sync.pairingProtocolFailed",
            Self::SyncConnectionFailed => "sync.connectionFailed",
            Self::SyncHostUnavailable => "sync.hostUnavailable",
            Self::SyncAuthorizationFailed => "sync.authorizationFailed",
            Self::SyncHostRejected => "sync.hostRejected",
            Self::SyncBindingMismatch => "sync.bindingMismatch",
            Self::SyncResourceMismatch => "sync.resourceMismatch",
            Self::SyncResourceUnavailable => "sync.resourceUnavailable",
            Self::SyncMutationRejected => "sync.mutationRejected",
            Self::SyncRateLimited => "sync.rateLimited",
            Self::SyncCheckpointMismatch => "sync.checkpointMismatch",
            Self::SyncProtocolFailed => "sync.protocolFailed",
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
    /// Return whether this successful cycle changed local catalog projections.
    fn sync_now(&self, role: SyncRole) -> Result<bool, SyncRuntimeError>;
    /// Read durable queue state after a cycle. Unknown telemetry stays absent.
    fn sync_progress(&self, _role: SyncRole) -> Result<SyncProgress, SyncRuntimeError> {
        Ok(SyncProgress::default())
    }
    /// Block until the paired host may have new commits, a local write queued
    /// work, the budget elapsed, or cancellation was requested. Peer-only:
    /// provisioners without a watch channel return
    /// [`WatchOutcome::Unsupported`] and the caller keeps waiting on the local
    /// waker alone. The default never blocks so simple adapters and tests keep
    /// their existing scheduling.
    fn watch_host_commits(
        &self,
        _cancelled: Arc<AtomicBool>,
        _wait: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        Ok(WatchOutcome::Unsupported)
    }
    /// Cancel an in-flight background cycle when the user pauses or stops the
    /// session. Provisioners with cancellable transports should override this;
    /// the default preserves compatibility with simple adapters.
    fn sync_now_cancellable(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
    ) -> Result<bool, SyncRuntimeError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(SyncRuntimeError::Busy);
        }
        self.sync_now(role)
    }
    /// Checkpoint updates belong only to this call; the runtime fences the
    /// observer when the cycle ends or its session is cancelled.
    fn sync_now_with_progress(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
        _progress: Option<ProgressNotifier>,
    ) -> Result<bool, SyncRuntimeError> {
        self.sync_now_cancellable(role, cancelled)
    }
    fn revoke_device(&self, device_id: &str) -> Result<(), SyncRuntimeError>;
    fn leave_sync(&self, root: LibraryRootId) -> Result<(), SyncRuntimeError>;
    fn recover_restored_host(
        &self,
        _root: LibraryRootId,
    ) -> Result<echo_core::application::ports::HostRecoveryArchive, SyncRuntimeError> {
        Err(SyncRuntimeError::Unconfigured)
    }
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

/// A callback can outlive a cancelled transport's blocking store write. Its
/// active token is private to one cycle and is revoked before final status is
/// read, so late checkpoints cannot overwrite completion or a later cycle.
struct CycleProgressObserver {
    notify: ProgressNotifier,
    active: Arc<AtomicBool>,
}

impl CycleProgressObserver {
    fn new(
        state: Arc<Mutex<Inner>>,
        notifier: Arc<Mutex<Option<StatusNotifier>>>,
        session_id: u64,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let cycle_active = Arc::clone(&active);
        let accepted_sequence = Mutex::new(0);
        let notify = Arc::new(move |sequence, progress: SyncProgress| {
            let mut accepted_sequence = accepted_sequence
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if sequence <= *accepted_sequence {
                return;
            }
            let status = {
                let mut inner = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !cycle_active.load(Ordering::Acquire)
                    || cancelled.load(Ordering::Acquire)
                    || inner.status.session_id != session_id
                    || !inner.status.enabled
                    || inner.status.state == SyncState::Paused
                {
                    return;
                }
                *accepted_sequence = sequence;
                inner.status.state = SyncState::Syncing;
                inner.status.uploaded_bytes = progress.uploaded_bytes;
                inner.status.total_upload_bytes = progress.total_upload_bytes;
                inner.status.downloaded_bytes = progress.downloaded_bytes;
                inner.status.total_download_bytes = progress.total_download_bytes;
                // Complete bytes are still awaiting publication into the library.
                inner.status.media_ready = Some(false);
                inner.status.catalog_changed = false;
                inner.status.event_sequence += 1;
                inner.status.clone()
            };
            drop(accepted_sequence);
            let notifier = notifier
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(notifier) = notifier {
                notifier(status);
            }
        });
        Self { notify, active }
    }
}

fn run_observed_cycle(
    provisioner: &dyn SyncProvisioner,
    role: SyncRole,
    cancelled: Arc<AtomicBool>,
    session_id: u64,
    state: Arc<Mutex<Inner>>,
    notifier: Arc<Mutex<Option<StatusNotifier>>>,
) -> (Result<bool, SyncRuntimeError>, SyncProgress) {
    let observer = CycleProgressObserver::new(state, notifier, session_id, Arc::clone(&cancelled));
    let result =
        provisioner.sync_now_with_progress(role, cancelled, Some(Arc::clone(&observer.notify)));
    drop(observer);
    let progress = provisioner.sync_progress(role).unwrap_or_default();
    (result, progress)
}

impl Drop for CycleProgressObserver {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
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
            inner.status.catalog_changed = false;
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
                            inner.status.catalog_changed = false;
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

    /// Restore the persisted LAN role after application startup. Peer sessions
    /// start their normal retrying sync loop, so an unavailable host is retried
    /// without blocking the application. Host mode binds the best available
    /// private LAN address.
    pub fn restore_saved_session(&self) -> Result<Option<SyncStatus>, SyncRuntimeError> {
        let Some(configuration) = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        else {
            return Ok(None);
        };
        if self.active_root()? != configuration.local_root {
            return Ok(None);
        }

        if configuration.host {
            let mut last_error = SyncRuntimeError::DiscoveryFailed;
            for address in available_local_lan_addresses() {
                match self.host_start(&address.to_string()) {
                    Ok(status) => return Ok(Some(status)),
                    Err(error) => last_error = error,
                }
            }
            Err(last_error)
        } else {
            self.peer_start().map(Some)
        }
    }

    /// Stop the current LAN session and remove this library's pairing while
    /// retaining its local catalog and media.
    pub fn leave_sync(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if self.status().enabled {
            self.stop_inner()?;
        }
        let root = self.active_root()?;
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
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
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
        let (result, progress) = run_observed_cycle(
            provisioner.as_ref(),
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
        inner.status.local_root = Some(root.as_uuid().to_string());
        inner.status.address = Some(address);
        inner.status.last_error = None;
        inner.status.catalog_changed = false;
        SyncProgress::default().apply(&mut inner.status);
        inner.root = Some(root);
    }

    fn change_state(&self, state: SyncState, error: Option<String>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.state = state;
        inner.status.last_error = error;
        inner.status.catalog_changed = false;
        inner.status.event_sequence += 1;
    }

    fn complete_cycle(&self, state: SyncState, catalog_changed: bool) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.state = state;
        inner.status.last_error = None;
        inner.status.catalog_changed = catalog_changed;
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
        // Queue the first cycle before the worker starts so its initial drain
        // always sees it: sending after the spawn would race the worker's
        // first check, and a peer that went straight into a held watch would
        // then sit on this hint until the watch ended.
        let _ = wake.send(());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("echo-lan-sync-loop".into())
            .spawn(move || {
                // Local SQLite writes wake the loop immediately. Between
                // cycles a healthy peer holds one long-poll against the host,
                // which answers as soon as it publishes; the plain waker wait
                // remains for hosts, retrying peers, and hosts without the
                // wait capability.
                let mut retry_delay = RETRY_BASE_DELAY;
                let mut retry_attempts = 0_u32;
                let mut retrying = false;
                // A recycled keep-alive connection and an unreachable host are
                // one failed attempt each; only the second consecutive failure
                // is evidence of the latter, so the first one is retried
                // silently instead of being published as a sync failure.
                let mut watch_failures = 0_u32;
                loop {
                    // A pending local wakeup — including the initial one sent
                    // when the loop starts — is work right now. A retrying peer
                    // still lets its backoff run first, matching the plain
                    // waker's suppression of early retries.
                    let mut woke_locally = false;
                    if !retrying {
                        while wakeups.try_recv().is_ok() {
                            woke_locally = true;
                        }
                    }
                    if !woke_locally {
                        // Wait for work. Only a healthy peer watches: a host
                        // has nothing remote to discover, and a retrying peer
                        // must let its backoff run instead of holding a dead
                        // connection.
                        if role == SyncRole::Peer && !retrying {
                            match provisioner
                                .watch_host_commits(Arc::clone(&worker_stop), PEER_WATCH_BUDGET)
                            {
                                Ok(WatchOutcome::Unsupported) => {
                                    watch_failures = 0;
                                    let delay = FALLBACK_POLL_INTERVAL;
                                    if !wait_for_sync_wakeup(
                                        &wakeups,
                                        delay,
                                        retrying,
                                        &worker_stop,
                                    ) {
                                        break;
                                    }
                                }
                                Ok(WatchOutcome::LocalWrite) => {
                                    watch_failures = 0;
                                    // The watch drained its own channel; clear
                                    // the loop's too so a later plain wait
                                    // cannot replay stale hints as extra cycles.
                                    while wakeups.try_recv().is_ok() {}
                                }
                                Ok(WatchOutcome::Advanced | WatchOutcome::BudgetElapsed) => {
                                    watch_failures = 0;
                                }
                                Err(SyncRuntimeError::Busy) => break,
                                Err(error) => {
                                    // Retry once without publishing: the host's
                                    // connection lifetime recycles idle
                                    // keep-alive sockets, which surfaces as a
                                    // transport error even though the peer is
                                    // healthy. A second consecutive failure is a
                                    // real outage and keeps the retry cadence.
                                    if watch_failures == 0 {
                                        watch_failures = 1;
                                        tracing::debug!(
                                            target: "echo_desktop::sync",
                                            error = ?error,
                                            "peer watch failed once; retrying before reporting"
                                        );
                                        continue;
                                    }
                                    // The host was unreachable: record it like a
                                    // failed cycle and let the retry cadence run.
                                    let keep_waiting = {
                                        let mut inner = state
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                                        if inner.status.session_id != session_id
                                            || !inner.status.enabled
                                        {
                                            false
                                        } else {
                                            inner.status.state = SyncState::Failed;
                                            inner.status.catalog_changed = false;
                                            inner.status.last_error =
                                                Some(error.message_key().into());
                                            inner.status.event_sequence += 1;
                                            true
                                        }
                                    };
                                    if !keep_waiting {
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
                                    retrying = true;
                                    if retry_attempts > 0 {
                                        retry_delay =
                                            (retry_delay * 2).min(Duration::from_secs(60));
                                    }
                                    retry_attempts = retry_attempts.saturating_add(1);
                                    continue;
                                }
                            }
                        } else {
                            let delay = if retrying {
                                jittered_retry_delay(retry_delay)
                            } else {
                                FALLBACK_POLL_INTERVAL
                            };
                            if !wait_for_sync_wakeup(&wakeups, delay, retrying, &worker_stop) {
                                break;
                            }
                        }
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
                            inner.status.catalog_changed = false;
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
                    let (result, progress) = {
                        let Ok(_cycle) = cycle.lock() else {
                            break;
                        };
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        run_observed_cycle(
                            provisioner.as_ref(),
                            role,
                            Arc::clone(&worker_stop),
                            session_id,
                            Arc::clone(&state),
                            Arc::clone(&notifier),
                        )
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
                        progress.apply(&mut inner.status);
                        match result {
                            Ok(catalog_changed) => {
                                inner.status.catalog_changed = catalog_changed;
                                inner.status.state = if role == SyncRole::Host {
                                    SyncState::Hosting
                                } else {
                                    SyncState::Connected
                                };
                                inner.status.last_error = None;
                                retrying = false;
                                retry_delay = RETRY_BASE_DELAY;
                                retry_attempts = 0;
                            }
                            Err(error) => {
                                inner.status.state = SyncState::Failed;
                                inner.status.catalog_changed = false;
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

pub fn available_local_lan_addresses() -> Vec<IpAddr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut addresses = interfaces
        .into_iter()
        .filter(|interface| {
            interface.is_oper_up()
                && !interface.is_loopback()
                && !interface.is_link_local()
                && !interface.is_p2p()
        })
        .map(|interface| interface.ip())
        .filter(|address| match address {
            IpAddr::V4(address) => address.is_private(),
            IpAddr::V6(address) => (address.segments()[0] & 0xfe00) == 0xfc00,
        })
        .collect::<Vec<_>>();
    addresses.sort_by_key(|address| match address {
        IpAddr::V4(address) if address.octets()[0] == 192 && address.octets()[1] == 168 => 0,
        IpAddr::V4(address) if address.octets()[0] == 10 => 1,
        IpAddr::V4(address)
            if address.octets()[0] == 172 && (16..=31).contains(&address.octets()[1]) =>
        {
            2
        }
        IpAddr::V4(_) => 3,
        IpAddr::V6(_) => 4,
    });
    addresses.dedup();
    addresses
}

#[cfg(test)]
#[path = "sync_runtime/tests.rs"]
mod tests;
