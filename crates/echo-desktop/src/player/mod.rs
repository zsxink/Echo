//! Desktop player adapter: libmpv actor, queue and playback session (task 8.x).
//!
//! libmpv is owned by a single-threaded actor. No FFI type crosses this module
//! boundary.
//!
//! **`unsafe` isolation:** `echo-core` forbids `unsafe_code` workspace-wide;
//! `echo-desktop` relaxes it only here (see its `[lints.rust]`), and every
//! *other* module of this crate re-forbids `unsafe`. Within `player`, [`ffi`]
//! is the single module that touches libmpv C types; [`actor`] calls safe
//! wrappers from `ffi`.
//!
//! The module is structured around the [`port::PlayerPort`] trait:
//!
//! - [`port`] — `PlayerPort`, `PlayerCommand`, `PlayerSnapshot`, `PlayMode`
//!   and `PlayerError`. The adapter boundary between the coordinator and the
//!   platform player actor.
//! - [`fake`] — `FakePlayer`, an in-process test double that processes
//!   commands synchronously. Coordinator, queue, statistics and
//!   platform-control tests use this to avoid loading libmpv.
//! - [`ffi`] — the isolated `unsafe` libmpv binding (dynamic loading, minimal
//!   symbol set).
//! - [`actor`] — the dedicated OS-thread actor that owns the libmpv handle.
//!
//! Future submodules (coordinator, queue, session) will be added as tasks
//! 8.5–8.11 land.

pub mod actor;
pub mod coordinator;
pub mod deletion;
pub mod fake;
pub mod ffi;
pub mod port;
pub mod queue;
pub mod recording;
pub mod session;
