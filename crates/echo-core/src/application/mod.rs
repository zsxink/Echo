//! Application layer: use cases, ports and transaction orchestration.
//!
//! Depends on [`crate::domain`]; never contains view logic or concrete
//! infrastructure implementations.

pub mod ports;

/// Test doubles for the ports ([`ports`]). Compiled only under `cargo test`
/// or the `testkit` feature so no fake leaks into a production build.
#[cfg(any(test, feature = "testkit"))]
pub mod testing;
