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
//! - [`recover`] — import crash recovery (`RecoverOperations`, the
//!   three-location existence/hash matrix; tasks 5.5 / 5.10).
//! - [`boot`] — the one pre-ready startup step (`BootRecovery`): recover the
//!   active root under a per-root scan exclusion and surface the readiness
//!   gate the runtime consults before starting the watcher/player (task 5.10).
//! - [`favorite`] — the favorite toggle with an authoritative committed
//!   snapshot (`SetFavorite`, task 6.3).
//! - [`detail`] — the read-only song detail DTO (`GetSongDetail`, task 6.4).

pub mod boot;
pub mod catalog;
pub mod delete;
pub mod detail;
pub mod favorite;
pub mod import;
pub mod playlist;
pub mod ports;
pub mod recover;
pub mod relink;
pub mod root_switch;
pub mod scan;
pub mod trash;
pub mod watch;

/// Test doubles for the ports ([`ports`]). Compiled only under `cargo test`
/// or the `testkit` feature so no fake leaks into a production build.
#[cfg(any(test, feature = "testkit"))]
pub mod testing;
