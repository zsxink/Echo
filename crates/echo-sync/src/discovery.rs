//! Opt-in mDNS advertisement and discovery for local Echo hosts.

use mdns_sd::{Receiver as MdnsReceiver, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;

pub const SERVICE_TYPE: &str = "_echo-sync._tcp.local.";

pub struct LanDiscovery {
    daemon: ServiceDaemon,
    events: MdnsReceiver<ServiceEvent>,
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
        Ok(Self { daemon, events })
    }

    /// Advertise one host endpoint on the selected LAN address. Only public
    /// connection metadata is placed in the TXT record; credentials and
    /// library paths are never announced.
    ///
    /// # Errors
    /// Propagates invalid service metadata and daemon browse or register failures.
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
        let events = daemon.browse(SERVICE_TYPE)?;
        daemon.register(info)?;
        Ok(Self { daemon, events })
    }

    #[must_use]
    pub const fn events(&self) -> &MdnsReceiver<ServiceEvent> {
        &self.events
    }

    /// Request shutdown of the discovery daemon.
    ///
    /// # Errors
    /// Propagates failures sending the daemon shutdown request.
    pub fn shutdown(self) -> Result<(), mdns_sd::Error> {
        self.daemon.shutdown()?;
        Ok(())
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
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
