//! Real-network mDNS tests for [`LanDiscovery`].
//!
//! These exercise the exact production pattern — a long-lived advertisement
//! plus fresh bounded browse sessions — and need a working multicast
//! environment (loopback is enough). They are `#[ignore]`d so the default
//! `cargo test` run stays hermetic; run them explicitly with:
//!
//! ```text
//! cargo test -p echo-sync --test discovery_network -- --ignored --nocapture
//! ```

use echo_sync::discovery::{LanDiscovery, SERVICE_TYPE};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Mirrors `sync_runtime::discovery::discover_hosts`: a fresh daemon, a bounded
/// window, and only fully resolved services count.
fn collect_hosts(window: Duration) -> BTreeMap<String, (Vec<String>, u16)> {
    let discovery = LanDiscovery::browse().expect("start browse");
    let events = discovery.events().expect("browse session carries events");
    let deadline = Instant::now() + window;
    let mut hosts = BTreeMap::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(remaining) else {
            break;
        };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
            hosts.insert(
                info.get_fullname().to_string(),
                (
                    info.get_addresses()
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    info.get_port(),
                ),
            );
        }
    }
    discovery.shutdown().expect("stop browse");
    hosts
}

/// Collect hosts with bounded retries.
///
/// A single browse session can legitimately miss a service when the shared
/// 5353 multicast group is busy (other apps' announcements fill a fresh
/// socket's receive buffer and the kernel drops the rest), so one empty
/// window is not proof of a dead responder. Retrying with fresh daemons
/// absorbs that ambient noise; it cannot mask the regression this suite
/// guards, because a daemon blocked on a full event channel stays silent
/// for the rest of the process — every retry would come back empty.
fn collect_hosts_with_retries(
    window: Duration,
    attempts: usize,
) -> BTreeMap<String, (Vec<String>, u16)> {
    let mut hosts = BTreeMap::new();
    for attempt in 1..=attempts {
        hosts = collect_hosts(window);
        if !hosts.is_empty() {
            return hosts;
        }
        eprintln!("browse attempt {attempt}/{attempts} saw no hosts, retrying");
    }
    hosts
}

fn unique_name(tag: &str) -> String {
    format!("EchoTest-{tag}-{}", std::process::id())
}

fn advertise(name: &str) -> LanDiscovery {
    LanDiscovery::advertise(
        name,
        "echo-host.local.",
        IpAddr::from([127, 0, 0, 1]),
        50826,
        "device",
        1,
    )
    .expect("advertise")
}

fn instance_name(name: &str) -> String {
    format!("{name}.{SERVICE_TYPE}")
}

#[test]
#[ignore = "needs a working mDNS multicast environment"]
fn advertised_service_is_discoverable_by_a_fresh_browser() {
    let name = unique_name("basic");
    let advertisement = advertise(&name);

    let hosts = collect_hosts_with_retries(Duration::from_secs(5), 3);

    assert!(
        hosts.contains_key(&instance_name(&name)),
        "a fresh browser must resolve the advertised host, saw: {hosts:?}"
    );
    advertisement.shutdown().expect("stop advertisement");
}

#[test]
#[ignore = "needs a working mDNS multicast environment"]
fn advertised_host_stays_discoverable_under_event_channel_pressure() {
    // Regression: `advertise` used to browse the service type with an event
    // channel nobody drained. Peer services fill that bounded channel, `mdns-sd`
    // then blocks its daemon thread on the next event, and the host silently
    // stops answering queries while still "running".
    let name = unique_name("pressure");
    let mut peers = Vec::new();
    for index in 0..6 {
        peers.push(advertise(&unique_name(&format!("peer{index}"))));
    }
    let advertisement = advertise(&name);

    // Let the peers' announcements fill the host daemon's browse channel before
    // any browser asks for the host.
    std::thread::sleep(Duration::from_secs(4));
    let hosts = collect_hosts_with_retries(Duration::from_secs(5), 3);

    assert!(
        hosts.contains_key(&instance_name(&name)),
        "the host must stay discoverable while peers announce, saw: {hosts:?}"
    );
    advertisement.shutdown().expect("daemon confirms shutdown");
    for peer in peers {
        peer.shutdown().expect("daemon confirms shutdown");
    }
}

#[test]
#[ignore = "needs a working mDNS multicast environment"]
fn shutdown_terminates_the_daemon() {
    let advertisement = advertise(&unique_name("shutdown"));
    // A clean shutdown confirms the daemon thread exited; sockets and thread
    // must not linger for the rest of the process.
    advertisement.shutdown().expect("daemon confirms shutdown");
}
