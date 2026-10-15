//! Bounded LAN service discovery. Advertisements are hints, never identity.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use echo_sync::discovery::LanDiscovery;

use super::{DiscoveredHost, SyncRuntimeError};

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
