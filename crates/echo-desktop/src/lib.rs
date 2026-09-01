//! Echo desktop adaptation layer.
//!
//! Sits between the Tauri shell and [`echo-core`]: it maps core domain data to
//! IPC DTOs, owns the libmpv player actor and platform adapters (tray, media
//! keys, file association, system trash, reveal), and orchestrates the
//! application runtime lifecycle.
//!
//! Layering: `ipc` and `player` depend on `echo-core`; `runtime` assembles the
//! pieces; `platform` adapters implement desktop-side ports.

// Durable-state docs explain *why* a write is atomic before* they say what the
// store holds; keep the prose readable over chasing a 100-char first line.
#![allow(clippy::too_long_first_doc_paragraph)]

pub mod ipc;
pub mod platform;
pub mod player;
pub mod runtime;
