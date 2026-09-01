//! Platform adapters: tray, media control, file association, system trash,
//! reveal-in-folder and window lifecycle (task 9.x).
//!
//! `unsafe` is confined to the `player` subtree (see crate README); this module
//! forbids it.
#![forbid(unsafe_code)]

/// Desktop observability: structured tracing, panic hook and safe error
/// mapping (task 7.8).
pub mod diagnostics;

pub mod dialogs;
/// Desktop local-state persistence (theme / close / window / playback session),
/// stored atomically and durably (task 7.6).
pub mod local_state;
/// Security posture: CSP, the custom cover-protocol boundary and the minimal
/// Tauri capability (task 7.7).
pub mod security;
