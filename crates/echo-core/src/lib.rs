//! Echo core library.
//!
//! `echo-core` is the cross-platform business core of Echo: models, library,
//! scanning, indexing, import, and persistence. It is intentionally free of
//! Tauri, mpv, Flutter, React, and any operating-system or UI dependency.
//!
//! The crate is organised in three layers (from the inside out):
//!
//! - [`domain`]: entities, value objects, state machines and invariants.
//! - [`application`]: use cases, ports and transaction orchestration.
//! - [`infrastructure`]: Adapters for `SQLite`, the file system, metadata parsing.
//!
//! Dependencies may only flow inward: `application` depends on `domain`;
//! `infrastructure` implements `application`'s ports. Nothing may depend
//! outward onto a UI or platform layer.

// Echo documents its privacy policy and invariants in descriptive first
// paragraphs that intentionally exceed clippy's style limit; the `-D warnings`
// CI flag would otherwise reject them. Scope the relaxation to this crate only.
#![allow(clippy::too_long_first_doc_paragraph)]

pub mod application;
pub mod domain;
pub mod infrastructure;

/// Error types shared across the core (task 2.5 classification).
pub mod error;

// Re-export the domain identities and error surfaces so callers — and the
// desktop layer — can name them without deep module paths.
pub use domain::ids::{
    LibraryRootId, OperationId, PlaybackSessionId, PlaylistId, QueueEntryId, RelativeMediaPath,
    Revision, SongId,
};

/// Logging privacy guard, diagnostic-mode flag and test-logger integration
/// (task 1.8). The desktop runtime owns global subscriber init in production.
pub mod logging;

pub use logging::{init_test_logger, redact_path, redact_sensitive};

/// Convenience re-export for callers that own the diagnostics-mode switch.
pub use logging::DiagnosticMode;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
