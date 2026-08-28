//! Echo desktop adaptation layer.
//!
//! Sits between the Tauri shell and [`echo-core`]: it maps core domain data to
//! IPC DTOs, owns the libmpv player actor and platform adapters (tray, media
//! keys, file association, system trash, reveal), and orchestrates the
//! application runtime lifecycle.
//!
//! Layering: `ipc` and `player` depend on `echo-core`; `runtime` assembles the
//! pieces; `platform` adapters implement desktop-side ports.

pub mod ipc;
pub mod platform;
pub mod player;
pub mod runtime;
