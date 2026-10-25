//! Opt-in native LAN lifecycle. Provisioning never accepts keys from the `WebView`.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use echo_core::application::ports::{DeviceIdProvider, LibraryRepository};
use echo_core::domain::ids::LibraryRootId;
use echo_core::domain::sync::{SyncBinding, SyncSessionIntent};
use echo_core::infrastructure::sqlite::SqliteDatabase;
pub use echo_desktop::ipc::sync::{
    DiscoveredHost, SyncPeer, SyncRole, SyncSessionIntent as IpcSyncSessionIntent, SyncState,
    SyncStatus,
};
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
    pub host_device_id: echo_core::domain::sync::DeviceId,
}

/// Native composition owns pairing, secure credentials and Core orchestration.
/// An absent adapter is explicitly unavailable; no memory-only credentials or
/// plaintext fallback are substituted for durable platform provisioning. The
/// provisioning surface (this trait) and the cycle driver it serves
/// ([`SyncCyclePort`]) are separate so adapters can differ: a session manager
/// only needs the lifecycle methods, the sync loop only the cycle methods.
pub trait SyncProvisioner: Send + Sync {
    fn host(&self, root: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError>;
    fn peer(&self, root: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError>;
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

/// One synchronization cycle against the paired host. The runtime owns this
/// capability independently from provisioning and lifecycle operations.
pub trait SyncCyclePort: Send + Sync {
    /// Return whether this successful cycle changed local catalog projections.
    fn sync_now(&self, role: SyncRole) -> Result<bool, SyncRuntimeError>;
    /// Read durable queue state after a cycle. Unknown telemetry stays absent.
    fn sync_progress(&self, _role: SyncRole) -> Result<SyncProgress, SyncRuntimeError> {
        Ok(SyncProgress::default())
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

/// Project the Core lifecycle intent onto the typed IPC enum. The two are kept
/// separate so a wire change never silently alters persistence semantics.
#[allow(clippy::redundant_pub_crate)] // shared by the `handoff`/test submodules
pub(super) const fn ipc_session_intent(intent: SyncSessionIntent) -> IpcSyncSessionIntent {
    match intent {
        SyncSessionIntent::Enabled => IpcSyncSessionIntent::Enabled,
        SyncSessionIntent::Stopped => IpcSyncSessionIntent::Stopped,
        SyncSessionIntent::Paused => IpcSyncSessionIntent::Paused,
    }
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
    cycle_port: &dyn SyncCyclePort,
    role: SyncRole,
    cancelled: Arc<AtomicBool>,
    session_id: u64,
    state: Arc<Mutex<Inner>>,
    notifier: Arc<Mutex<Option<StatusNotifier>>>,
) -> (Result<bool, SyncRuntimeError>, SyncProgress) {
    let observer = CycleProgressObserver::new(state, notifier, session_id, Arc::clone(&cancelled));
    let result =
        cycle_port.sync_now_with_progress(role, cancelled, Some(Arc::clone(&observer.notify)));
    drop(observer);
    let progress = cycle_port.sync_progress(role).unwrap_or_default();
    (result, progress)
}

impl Drop for CycleProgressObserver {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

pub struct SyncRuntime {
    database: Arc<SqliteDatabase>,
    /// Session lifecycle and provisioning collaborator.
    provisioner: Option<Arc<dyn SyncProvisioner>>,
    /// Cycle execution collaborator.
    cycle_port: Option<Arc<dyn SyncCyclePort>>,
    // Serialize transitions independently from status. Status never waits for
    // TLS shutdown, discovery, SQLite, or a remote round trip.
    transition: Mutex<()>,
    cycle: Arc<Mutex<()>>,
    status_notifier: Arc<Mutex<Option<StatusNotifier>>>,
    inner: Arc<Mutex<Inner>>,
    restore_retry: Mutex<RestoreRetryState>,
}

struct RestoreRetryState {
    generation: Arc<AtomicU64>,
    worker: Option<RestoreRetryWorker>,
    /// Attempts already spent by the running or last-finished automatic
    /// restoration. Visible through [`SyncStatus::restore_attempts`].
    attempts: u32,
}

struct RestoreRetryWorker {
    wake: crate::restore_retry::RestoreRetryWake,
    thread: JoinHandle<()>,
}

mod control;
pub use handoff::RootHandoff;

mod handoff;
mod pairing;
mod session;
mod sync_loop;

impl SyncRuntime {
    /// Begin the bounded automatic restoration of the selected library.
    pub(super) fn start_restore_retry(self: &Arc<Self>) -> Result<(), SyncRuntimeError> {
        self.start_restore_retry_with_delays(
            crate::restore_retry::MAX_ATTEMPTS,
            crate::restore_retry::BASE_DELAY,
            crate::restore_retry::MAX_DELAY,
        )
    }

    /// Retry a restoration that already gave up. This is an explicit user
    /// action, so it always starts a fresh bounded attempt series instead of
    /// silently resuming behind the user's back.
    pub fn retry_restore(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if self.status().enabled {
            return Err(SyncRuntimeError::Busy);
        }
        // Supersede the previous worker like any other user transition:
        // exhaustion is published just before that thread exits, so an
        // immediate retry can race a not-yet-finished worker, and the restart
        // that follows must not be silently dropped behind it. Cancel-joining
        // also advances the generation, per the restore-retry contract that an
        // explicit retry fences any late attempt.
        self.cancel_restore_retry();
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            inner.status.state = SyncState::Disabled;
            inner.status.last_error = None;
            inner.status.event_sequence += 1;
        }
        Ok(self.status())
    }

    fn start_restore_retry_with_delays(
        self: &Arc<Self>,
        max_attempts: u32,
        base_delay: Duration,
        max_delay: Duration,
    ) -> Result<(), SyncRuntimeError> {
        let root = self.active_root()?;
        self.database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .filter(|configuration| {
                configuration.local_root == root
                    && configuration.session_intent == SyncSessionIntent::Enabled
            })
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let mut retry = self
            .restore_retry
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if retry
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.thread.is_finished())
        {
            return Ok(());
        }
        if let Some(worker) = retry.worker.take() {
            worker
                .thread
                .join()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        let generation = retry.generation.fetch_add(1, Ordering::AcqRel) + 1;
        retry.attempts = 0;
        let (wake, wakeups) = crate::restore_retry::RestoreRetryWake::new();
        // Release the retry-state lock before the worker starts: the worker
        // reads the same generation and attempts, so holding it here would
        // serialize the first attempt behind its own spawn.
        drop(retry);
        let runtime = Arc::downgrade(self);
        let thread = thread::Builder::new()
            .name("echo-sync-startup-retry".into())
            .spawn(move || {
                let mut delay = base_delay;
                for attempt in 1..=max_attempts {
                    if crate::restore_retry::wait_for_restore_retry(&wakeups, delay) {
                        // Interrupted by a user transition or producer drop.
                        return;
                    }
                    let Some(runtime) = runtime.upgrade() else {
                        return;
                    };
                    if !runtime.restore_retry_is_current(generation, root) {
                        // Superseded: a user transition or root change owns the
                        // session now. Never publish a stale failure.
                        return;
                    }
                    runtime.record_restore_attempt(attempt);
                    match runtime.restore_saved_session() {
                        Ok(Some(_)) => {
                            // A successful start owns the session; clear the
                            // retry accounting so status stops advertising it.
                            runtime.clear_restore_retry_state();
                            return;
                        }
                        Ok(None) => {
                            runtime.clear_restore_retry_state();
                            return;
                        }
                        Err(error) => {
                            let status =
                                runtime.report_restore_failure(error, attempt, max_attempts);
                            runtime.notify_status(status);
                            if attempt == max_attempts {
                                runtime.mark_restore_exhausted();
                                return;
                            }
                            delay = crate::restore_retry::next_delay(delay).min(max_delay);
                        }
                    }
                }
            })
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.restore_retry
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .worker = Some(RestoreRetryWorker { wake, thread });
        Ok(())
    }

    /// Whether one restoration attempt still belongs to the current library and
    /// user intent. Kept as a method so the worker never holds the retry lock
    /// across its own decisions.
    fn restore_retry_is_current(&self, generation: u64, root: LibraryRootId) -> bool {
        let current_generation = self
            .restore_retry
            .lock()
            .map_or(u64::MAX, |retry| retry.generation.load(Ordering::Acquire));
        let intent_enabled = self
            .database
            .current_sync_configuration()
            .ok()
            .flatten()
            .is_some_and(|configuration| {
                configuration.local_root == root
                    && configuration.session_intent == SyncSessionIntent::Enabled
            });
        crate::restore_retry::retry_allowed(
            generation,
            current_generation,
            self.active_root().ok() == Some(root),
            intent_enabled,
        )
    }

    fn record_restore_attempt(&self, attempt: u32) {
        if let Ok(mut retry) = self.restore_retry.lock() {
            retry.attempts = attempt;
        }
    }

    /// Stop and join any restore worker, advancing its generation so a delayed
    /// attempt can never act on a superseded root or user intent.
    pub(super) fn cancel_restore_retry(&self) {
        let worker = {
            let Ok(mut retry) = self.restore_retry.lock() else {
                return;
            };
            retry.generation.fetch_add(1, Ordering::AcqRel);
            retry.attempts = 0;
            retry.worker.take()
        };
        if let Some(mut worker) = worker {
            worker.wake.cancel();
            let _ = worker.thread.join();
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.status.restore_attempts = None;
            inner.status.restore_exhausted = false;
        }
    }

    fn clear_restore_retry_state(&self) {
        if let Ok(mut retry) = self.restore_retry.lock() {
            retry.attempts = 0;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.status.restore_attempts = None;
            inner.status.restore_exhausted = false;
        }
    }

    fn report_restore_failure(
        &self,
        error: SyncRuntimeError,
        attempt: u32,
        max_attempts: u32,
    ) -> SyncStatus {
        let status = self.report_error(error);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.status.restore_attempts = Some(attempt.min(max_attempts));
        drop(inner);
        status
    }

    fn mark_restore_exhausted(&self) {
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.status.restore_exhausted = true;
            inner.status.restore_attempts = None;
            inner.status.state = SyncState::Failed;
            inner.status.last_error = Some(
                SyncRuntimeError::SyncHostUnavailable
                    .message_key()
                    .to_owned(),
            );
            inner.status.event_sequence += 1;
        }
        // Allow a later app restart to try again only while the persisted
        // intent stays enabled; exhaustion itself is not persisted.
        self.notify_status(self.status());
        if let Ok(mut retry) = self.restore_retry.lock() {
            retry.worker = None;
        }
    }

    fn notify_status(&self, status: SyncStatus) {
        if let Some(notifier) = self
            .status_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            notifier(status);
        }
    }

    pub fn new(
        database: Arc<SqliteDatabase>,
        provisioner: Option<Arc<dyn SyncProvisioner>>,
        cycle_port: Option<Arc<dyn SyncCyclePort>>,
    ) -> Self {
        Self {
            database,
            provisioner,
            cycle_port,
            transition: Mutex::new(()),
            cycle: Arc::new(Mutex::new(())),
            status_notifier: Arc::new(Mutex::new(None)),
            inner: Arc::new(Mutex::new(Inner::default())),
            restore_retry: Mutex::new(RestoreRetryState {
                generation: Arc::new(AtomicU64::new(0)),
                worker: None,
                attempts: 0,
            }),
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
        // Intent is persisted per root and survives restart, so it can never be
        // inferred from a live session. A session's own root owns the intent
        // while it runs; otherwise the selected root's persisted choice does.
        if inner.root.is_none() {
            if let Ok(Some(configuration)) = self.database.current_sync_configuration() {
                if Some(configuration.local_root) == self.active_root().ok() {
                    inner.status.session_intent = ipc_session_intent(configuration.session_intent);
                }
            }
        }
        inner.status.clone()
    }
}

impl SyncRuntime {
    pub(super) fn persist_session_intent(
        &self,
        intent: echo_core::domain::sync::SyncSessionIntent,
    ) -> Result<(), SyncRuntimeError> {
        self.database
            .set_sync_session_intent(intent)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
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
        self.cancel_restore_retry();
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
use control::wait_for_sync_wakeup;

#[cfg(test)]
mod tests;
