//! Bounded LAN service discovery. Advertisements are hints, never identity.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use echo_sync::discovery::LanDiscovery;

use super::{DiscoveredHost, SyncRuntimeError};

/// Discovery runs only for this user-requested bounded interval. Public mDNS
/// metadata is never accepted as TLS identity or pairing authorization.
///
/// The window spans the initial query plus `mdns-sd`'s first retransmissions
/// (1s, 2s, 4s), so a single lost multicast query must not empty the result.
const DISCOVERY_WINDOW: Duration = Duration::from_secs(5);
// A busy LAN can overflow the fresh mDNS daemon's receive queue with unrelated
// multicast announcements. Match the network-discovery integration coverage:
// retry with a fresh daemon so one dropped query does not produce an empty UI.
const DISCOVERY_ATTEMPTS: usize = 3;

pub fn discover_hosts() -> Result<Vec<DiscoveredHost>, SyncRuntimeError> {
    for attempt in 1..=DISCOVERY_ATTEMPTS {
        let discovery = LanDiscovery::browse().map_err(|_| SyncRuntimeError::DiscoveryFailed)?;
        let Some(events) = discovery.events() else {
            // A browse session always carries its event channel; without one
            // the daemon could never report anything, so stop it and fail.
            let _ = discovery.shutdown();
            return Err(SyncRuntimeError::DiscoveryFailed);
        };
        let mut hosts = BTreeMap::new();
        let deadline = Instant::now() + DISCOVERY_WINDOW;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = events.recv_timeout(remaining) else {
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
        // The collected hosts answer the user's query. A daemon that cannot
        // confirm its exit is logged, without discarding those results.
        if let Err(error) = discovery.shutdown() {
            tracing::warn!(
                target: "echo_sync::discovery",
                error = %error,
                attempt,
                "LAN discovery daemon did not shut down cleanly"
            );
        }
        if !hosts.is_empty() {
            return Ok(hosts.into_values().collect());
        }
        tracing::debug!(
            target: "echo_sync::discovery",
            attempt,
            attempts = DISCOVERY_ATTEMPTS,
            "LAN discovery attempt found no hosts"
        );
    }
    Ok(Vec::new())
}
