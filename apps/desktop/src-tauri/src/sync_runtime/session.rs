//! Starting/stopping the host listener and the peer runtime, restoring saved sessions.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SyncRuntime {
    pub fn host_start(&self, address: &str) -> Result<SyncStatus, SyncRuntimeError> {
        let address = selected_lan_address(address)?;
        let _transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.start_host_locked(address)
    }

    pub(super) fn start_host_locked(
        &self,
        address: IpAddr,
    ) -> Result<SyncStatus, SyncRuntimeError> {
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
}
