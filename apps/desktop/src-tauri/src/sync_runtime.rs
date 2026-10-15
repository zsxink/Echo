//! Opt-in native LAN lifecycle. Provisioning never accepts keys from the `WebView`.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncRuntimeError {
    Unconfigured,
    InvalidAddress,
    LibraryUnavailable,
    Busy,
    BindFailed,
    DiscoveryFailed,
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
            inner: Arc::new(Mutex::new(Inner::default())),
        }
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
        let listener = TcpListener::bind(SocketAddr::new(address, 0))
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
        let (root, address) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            (inner.root, inner.host_address)
        };
        if Some(self.active_root()?) != root {
            self.stop_inner()?;
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.stop_inner()?;
        if status.role == SyncRole::Host {
            self.start_host_locked(address.ok_or(SyncRuntimeError::InvalidAddress)?)
        } else {
            self.start_peer_locked()
        }
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
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let status = self.status();
        if !status.enabled || status.state == SyncState::Paused {
            return Err(SyncRuntimeError::Unconfigured);
        }
        let root = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .root;
        if Some(self.active_root()?) != root {
            self.stop_inner()?;
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let provisioner = self
            .provisioner
            .as_ref()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        self.change_state(SyncState::Syncing, None);
        match provisioner.sync_now(status.role) {
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
        api.create_pairing_invitation(host_device, certificate, Duration::from_secs(300))
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

    fn stop_inner(&self) -> Result<(), SyncRuntimeError> {
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
        drop(inner);
        pairing_result
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

/// Discovery runs only for this user-requested bounded interval. Public mDNS
/// metadata is never accepted as TLS identity or pairing authorization.
pub fn discover_hosts() -> Result<Vec<DiscoveredHost>, SyncRuntimeError> {
    let discovery = LanDiscovery::browse().map_err(|_| SyncRuntimeError::DiscoveryFailed)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut hosts = BTreeMap::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = discovery.events().recv_timeout(remaining) else {
            break;
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
            let name = info.get_fullname().to_owned();
            hosts.insert(
                name.clone(),
                DiscoveredHost {
                    name,
                    addresses: info
                        .get_addresses()
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    port: info.get_port(),
                    device_id: info
                        .get_properties()
                        .get_property_val_str("device")
                        .map(str::to_owned),
                    protocol_version: info
                        .get_properties()
                        .get_property_val_str("protocol")
                        .map(str::to_owned),
                },
            );
        }
    }
    discovery
        .shutdown()
        .map_err(|_| SyncRuntimeError::DiscoveryFailed)?;
    Ok(hosts.into_values().collect())
}

fn selected_lan_address(address: &str) -> Result<IpAddr, SyncRuntimeError> {
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
mod tests {
    use super::*;

    #[test]
    fn unconfigured_runtime_stays_offline_and_stop_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let database =
            Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
        let runtime = SyncRuntime::new(database, None);
        assert_eq!(runtime.status().state, SyncState::Disabled);
        assert_eq!(
            runtime.host_start("192.168.1.2").unwrap_err(),
            SyncRuntimeError::Unconfigured
        );
        assert_eq!(
            runtime.peer_start().unwrap_err(),
            SyncRuntimeError::Unconfigured
        );
        assert_eq!(
            runtime.sync_now().unwrap_err(),
            SyncRuntimeError::Unconfigured
        );
        assert!(!runtime.status().enabled);
        assert_eq!(runtime.stop().unwrap().state, SyncState::Disabled);
        assert_eq!(runtime.stop().unwrap().state, SyncState::Disabled);
    }

    #[test]
    fn host_address_cannot_bind_wildcards_public_or_loopback_interfaces() {
        for address in [
            "0.0.0.0",
            "127.0.0.1",
            "8.8.8.8",
            "224.0.0.1",
            "::",
            "::1",
            "2001:4860:4860::8888",
            "hostname",
        ] {
            assert_eq!(
                selected_lan_address(address),
                Err(SyncRuntimeError::InvalidAddress)
            );
        }
        for address in [
            "192.168.1.2",
            "10.0.0.1",
            "172.16.0.2",
            "169.254.1.2",
            "fd00::1",
        ] {
            assert!(selected_lan_address(address).is_ok());
        }
    }
}
