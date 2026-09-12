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
/// Single-file import source reader for "import current temporary playback item"
/// (task 11.7): imports a desktop-owned absolute path into the active library
/// without the WebView ever receiving that path.
pub mod import;
/// Desktop local-state persistence (theme / close / window / playback session),
/// stored atomically and durably (task 7.6).
pub mod local_state;
/// Media-control seam (macOS Now Playing / Windows SMTC / Linux MPRIS): key
/// vocabulary, exactly-one command mapping and the degrade path (task 9.4).
pub mod media_control;
/// Reveal-in-folder fallback policy: parent-directory fallback when the
/// platform cannot locate the specific row (Linux) (task 9.5).
pub mod reveal;
/// Security posture: CSP, the custom cover-protocol boundary and the minimal
/// Tauri capability (task 7.7).
pub mod security;
/// macOS menu-bar / Windows-Linux tray adapter logic: summary lines, play/pause
/// label and command mapping (task 9.3).
pub mod status_menu;
/// System-trash boundary: Windows file-lock retry policy and the
/// irreversibility contract over an injected OS backend (task 9.5).
pub mod trash;
