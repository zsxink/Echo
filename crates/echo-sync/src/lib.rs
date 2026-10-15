//! LAN transport adapters for the Echo synchronization use cases.
//!
//! This crate is the Infrastructure boundary: protocol and network adapters
//! depend inward on `echo-core`; the Core does not depend on this crate. Network
//! capabilities are opt-in features so ordinary desktop builds remain offline.

#![forbid(unsafe_code)]

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "discovery")]
pub mod discovery;
pub mod pairing;
pub mod protocol;
#[cfg(feature = "client")]
pub mod replica;
pub mod resource_transfer;
#[cfg(feature = "server")]
pub mod server;

/// Whether the HTTP client adapter is present in this build.
pub const CLIENT_ENABLED: bool = cfg!(feature = "client");
/// Whether the HTTP server adapter is present in this build.
pub const SERVER_ENABLED: bool = cfg!(feature = "server");
/// Whether mDNS discovery is present in this build.
pub const DISCOVERY_ENABLED: bool = cfg!(feature = "discovery");
