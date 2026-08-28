//! Application layer: use cases, ports and transaction orchestration.
//!
//! Depends on [`crate::domain`]; never contains view logic or concrete
//! infrastructure implementations.
//!
//! Use cases (phase 4/5):
//!
//! - [`scan`] — the generation-driven scan pipeline (`StartScan`/`CancelScan`),
//!   including the size/mtime fast-skip, BLAKE3 hashing, batched reconcile
//!   and progress persistence.
//! - [`relink`] — the pure identity-resolution rules (path/hash/music-key).
//! - [`root_switch`] — the active-root candidate barrier
//!   (`PrepareLibraryCandidate`/`ActivateLibrary`).
//! - [`watch`] — file-event reconciliation (`ReconcileFsChanges`).
//! - [`import`] — per-input multi-select import planning (`PlanImport`,
//!   reserved OperationId/SongId, isolated per-input results).

pub mod import;
pub mod ports;
pub mod relink;
pub mod root_switch;
pub mod scan;
pub mod watch;

/// Test doubles for the ports ([`ports`]). Compiled only under `cargo test`
/// or the `testkit` feature so no fake leaks into a production build.
#[cfg(any(test, feature = "testkit"))]
pub mod testing;
