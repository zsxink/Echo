//! Opt-in mDNS advertisement and discovery for local Echo hosts.

use mdns_sd::{
    DaemonEvent, DaemonStatus, Receiver as MdnsReceiver, ServiceDaemon, ServiceEvent, ServiceInfo,
};
use std::net::IpAddr;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const SERVICE_TYPE: &str = "_echo-sync._tcp.local.";

/// Upper bound for waiting on the daemon thread to exit after a shutdown
/// request. A daemon that cannot confirm in time still holds its multicast
/// sockets, so callers learn about it instead of assuming a clean teardown.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

pub struct LanDiscovery {
    daemon: ServiceDaemon,
    /// Service events of a [`LanDiscovery::browse`] session. An
    /// [`LanDiscovery::advertise`] session deliberately never browses, so it
    /// carries no receiver.
    events: Option<MdnsReceiver<ServiceEvent>>,
}

impl LanDiscovery {
    /// Start browsing. Calling this method explicitly opts the process into
    /// multicast DNS; merely linking the discovery feature performs no I/O.
    ///
    /// # Errors
    /// Propagates failures creating the mDNS daemon or starting its browse.
    pub fn browse() -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE_TYPE)?;
        Ok(Self {
            daemon,
            events: Some(events),
        })
    }

    /// Advertise one host endpoint on the selected LAN address. Only public
    /// connection metadata is placed in the TXT record; credentials and
    /// library paths are never announced.
    ///
    /// The daemon answers queries from its registry and never browses:
    /// registration itself announces the service twice, which is all a
    /// responder needs. Browsing here would fill a bounded event channel that
    /// no caller drains; `mdns-sd` blocks its daemon thread once that channel
    /// is full, silencing the host on the network while it still appears to be
    /// running.
    ///
    /// # Errors
    /// Propagates invalid service metadata, daemon monitor or register
    /// failures, and failures spawning the diagnostics thread.
    pub fn advertise(
        name: &str,
        host_name: &str,
        address: IpAddr,
        port: u16,
        device_id: &str,
        protocol_version: u16,
    ) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let protocol = protocol_version.to_string();
        let properties = [("device", device_id), ("protocol", protocol.as_str())];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            name,
            host_name,
            address,
            port,
            &properties[..],
        )?;
        // Subscribe before registering so the first announcements are observed.
        let diagnostics = daemon.monitor()?;
        daemon.register(info)?;
        spawn_diagnostics(diagnostics, name)?;
        Ok(Self {
            daemon,
            events: None,
        })
    }

    /// Service events of a browse session, or `None` for an advertisement.
    #[must_use]
    pub const fn events(&self) -> Option<&MdnsReceiver<ServiceEvent>> {
        self.events.as_ref()
    }

    /// Request shutdown of the discovery daemon and wait for its thread to
    /// exit.
    ///
    /// Waiting matters: a shutdown request that is never confirmed leaves the
    /// daemon thread and its multicast sockets alive for the rest of the
    /// process.
    ///
    /// # Errors
    /// Propagates failures sending the shutdown request, and reports
    /// [`mdns_sd::Error::Again`] when the daemon thread does not exit within
    /// [`SHUTDOWN_TIMEOUT`].
    pub fn shutdown(self) -> Result<(), mdns_sd::Error> {
        let status = self.daemon.shutdown()?;
        match status.recv_timeout(SHUTDOWN_TIMEOUT) {
            Ok(DaemonStatus::Shutdown) => Ok(()),
            // Any other outcome means the thread is gone without confirming or
            // never exited; both leave the caller's assumption of a clean
            // teardown broken.
            _ => Err(mdns_sd::Error::Again),
        }
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        // Best effort: the diagnostics thread ends when the daemon's senders
        // drop, and the OS releases the sockets with the process.
        if let Ok(status) = self.daemon.shutdown() {
            let _ = status.recv_timeout(SHUTDOWN_TIMEOUT);
        }
    }
}

/// Logs daemon diagnostics until the daemon exits.
///
/// `mdns-sd` delivers these on a bounded channel and silently discards the
/// overflow, so without this drain a host that never announces — or a daemon
/// that failed to open its sockets — is indistinguishable from a healthy one.
fn spawn_diagnostics(
    events: MdnsReceiver<DaemonEvent>,
    service: &str,
) -> Result<JoinHandle<()>, mdns_sd::Error> {
    let service = service.to_owned();
    thread::Builder::new()
        .name("echo-lan-mdns-diag".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                log_daemon_event(&service, &event);
            }
        })
        .map_err(|error| mdns_sd::Error::Msg(format!("spawn diagnostics thread: {error}")))
}

fn log_daemon_event(service: &str, event: &DaemonEvent) {
    match event {
        DaemonEvent::Announce(name, interfaces) => {
            tracing::debug!(
                target: "echo_sync::discovery",
                service,
                name = %name,
                interfaces = %interfaces,
                "mDNS daemon announced the host service"
            );
        }
        DaemonEvent::Error(error) => {
            tracing::warn!(
                target: "echo_sync::discovery",
                service,
                error = %error,
                "mDNS daemon reported an error"
            );
        }
        DaemonEvent::NameChange(change) => {
            tracing::warn!(
                target: "echo_sync::discovery",
                service,
                original = %change.original,
                "mDNS daemon renamed the host service after a name conflict"
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcement_service_metadata_is_constructible_without_starting_daemon() {
        let properties = [("device", "device-uuid"), ("protocol", "1")];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "Echo Library",
            "echo-host.local.",
            "192.168.1.2",
            43210,
            &properties[..],
        )
        .unwrap();
        assert_eq!(info.get_port(), 43210);
        assert!(info
            .get_properties()
            .get_property_val_str("device")
            .is_some());
        assert!(info
            .get_properties()
            .get_property_val_str("token")
            .is_none());
        assert!(SERVICE_TYPE.starts_with("_echo-sync."));
    }
}
