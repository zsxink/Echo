//! Core error classification (task 2.5).
//!
//! Echo classifies every failure through a small, matchable enum so use cases
//! can branch on the *kind* of problem without parsing free-text messages. The
//! classification follows `docs/DESIGN.md` §17 and the `openspec` change §2.5:
//!
//! | Variant | Meaning | Example |
//! |---|---|---|
//! | `Validation` | caller passed a bad value | malformed UUID, unsafe relative path |
//! | `Permission` | OS/user denied access | unreadable root, unwritable staging dir |
//! | `Unavailable` | resource not present/disconnected | root unmounted, db locked away |
//! | `Conflict` | concurrent edit or state conflict | duplicate name, journal claim taken |
//! | `UnsupportedMedia` | format outside the supported matrix | `.wma` probe rejected |
//! | `CorruptMedia` | supported container is damaged | truncated FLAC, bad ID3 |
//! | `Io` | underlying I/O failure with an absolute path | rename failed on `…/x.flac` |
//! | `Storage` | database/storage-layer failure | migration failure, disk full |
//! | `Cancelled` | operation was cancelled | scan cancelled, user aborted |
//! | `InvariantViolation` | a documented invariant was broken | two active roots |
//!
//! Design rules honoured here:
//!
//! - **Infrastructure errors keep their `source`.** A wrapped `std::io::Error`
//!   or `rusqlite::Error` is always reachable via `source()`/`#[source]` and is
//!   logged at debug level; never dropped.
//! - **Public errors never leak absolute paths.** Paths live in a private field
//!   and are only exposed as a redacted `file-name (hash)` form through
//!   [`Error::to_log`]. `Display` shows the redacted form; the raw path is only
//!   reachable through the explicit, opt-in `diagnostic_origin()`.
//! - **Errors are `Send + Sync`**, so they can cross the desktop runtime's
//!   actor/channel boundaries.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::logging::{redact_path, redact_sensitive, DiagnosticMode};

/// Source of a validation failure, boxed so the enum stays small.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// What a `Validation` error failed on.
#[derive(Debug)]
pub enum ValidationSubject {
    /// A domain identifier / UUID value.
    Id,
    /// A relative media path.
    Path,
    /// A playlist or song name (grapheme/NFKC rules).
    Name,
    /// A search query parameter.
    Query,
    /// Any other validated input.
    Other,
}

/// Top-level error classification for the Echo core.
///
/// See the module docs for the variant semantics and the design rules
/// (source preservation, path redaction, `Send + Sync`).
#[derive(Debug, Error)]
pub enum Error {
    /// The caller supplied a value that fails validation (unambiguous, the
    /// bad field is named). Never wraps a run-time environment problem.
    #[error(
        "validation failed on {subject:?} ({field}): {}",
        redact_display(reason)
    )]
    Validation {
        /// The input category that failed.
        subject: ValidationSubject,
        /// Human-readable field name (e.g. `SongId`, `relative_path`).
        field: String,
        /// Why the value is invalid.
        reason: String,
        /// Optional upstream parse/validation error.
        source: Option<BoxedSource>,
    },

    /// An operation is not allowed for the current user/permissions.
    #[error("permission denied: {} ({kind})", redact_display(operation))]
    Permission {
        /// What was being attempted.
        operation: String,
        /// The kind of permission problem (hash only, never path data).
        kind: PermKind,
        /// Upstream OS error when available.
        #[source]
        source: Option<std::io::Error>,
    },

    /// A required resource (library root, file, filesystem) is not
    /// available right now. Distinguished from `Io` (definitive I/O failure)
    /// by the retry/back-off semantics: `Unavailable` is transient.
    #[error("resource unavailable: {}", redact_display(resource))]
    Unavailable {
        /// What is unavailable (redacted for logs).
        resource: String,
        /// Human-readable explanation for the UI (message key).
        hint: String,
        /// Upstream error observed through the failing call (e.g. the
        /// `stat`/`read_dir` that detected the unmounted root), kept for
        /// diagnostics; never reaches a log raw.
        #[source]
        source: Option<BoxedSource>,
    },

    /// The requested change conflicts with current state (duplicate identity,
    /// concurrent edit, journal claim already taken…). Safe to retry with new
    /// input; never overwrite silently.
    #[error("conflict: {}", redact_display(what))]
    Conflict {
        /// What conflicted (e.g. `playlist name already taken`).
        what: String,
        /// Optional upstream error (e.g. unique-constraint violation).
        source: Option<BoxedSource>,
    },

    /// The media format is outside the supported matrix (not a corruption —
    /// the file may be perfectly fine, just not a supported type).
    #[error(
        "unsupported media: {} ({})",
        redact_display(operation),
        redact_display(reason)
    )]
    UnsupportedMedia {
        /// What was being attempted.
        operation: String,
        /// Why it is unsupported.
        reason: String,
    },

    /// A supported container/stream is damaged or unreadable past recovery.
    #[error(
        "corrupt media: {} ({})",
        redact_display(operation),
        redact_display(reason)
    )]
    CorruptMedia {
        /// What was being attempted.
        operation: String,
        /// Diagnostic detail (never file content).
        reason: String,
    },

    /// Underlying I/O failure carrying the absolute path that failed. The path
    /// is kept internally for the caller to act on but must never reach a log
    /// line raw (see [`Error::to_log`]).
    #[error("i/o error on {} ({operation})", redact_path(path))]
    Io {
        /// The operation that failed.
        operation: String,
        /// The underlying I/O error (always kept).
        #[source]
        source: std::io::Error,
        /// Absolute path the operation was working on. Never logged raw.
        path: PathBuf,
    },

    /// Database/storage-layer failure (migration, corruption, disk).
    #[error("storage error: {}", redact_display(what))]
    Storage {
        /// Short classifier (e.g. `migration`, `integrity`).
        what: String,
        /// Upstream error (rusqlite, io, …), kept.
        #[source]
        source: BoxedSource,
    },

    /// The operation was cancelled before completion. Distinct from `Conflict`
    /// and `Unavailable`: this is a deliberate user or supervisor abort, so
    /// callers shouldn't retry automatically.
    #[error("operation cancelled")]
    Cancelled,

    /// A documented domain invariant was violated. Only used for genuine
    /// internal corruption / programmer error, never for user-input failures.
    #[error("invariant violation: {}", redact_display(why))]
    InvariantViolation {
        /// Which invariant (e.g. `at most one active root`).
        why: String,
    },
}

impl Error {
    /// Redacted, structured, path-free log line for this error.
    ///
    /// Default (`DiagnosticMode::Off`) emits only error code + a redacted
    /// location (`file-name (hash)`) + unsensitive fields. When diagnostics
    /// are `On`, the caller (desktop runtime only) may opt in to the full
    /// path. Free-text fields (`reason`, resource names) are scrubbed so lyric
    /// text, tag strings, payloads or stray absolute spans never survive.
    ///
    /// # Panics
    ///
    /// Writing to a `Vec` cannot fail.
    #[must_use]
    pub fn to_log(&self, diagnostic: DiagnosticMode) -> String {
        let mut out = Vec::with_capacity(96);
        match self {
            Self::Validation { field, reason, .. } => {
                out.extend_from_slice(&log_two(
                    "validation",
                    "field",
                    field,
                    "reason",
                    &scrub_text(reason),
                ));
            }
            Self::Permission {
                operation, kind, ..
            } => {
                out.extend_from_slice(&log_two(
                    "permission",
                    "operation",
                    &scrub_operation(operation),
                    "kind",
                    aspect(*kind),
                ));
            }
            Self::Unavailable { resource, hint, .. } => {
                out.extend_from_slice(&log_two(
                    "unavailable",
                    "resource",
                    &scrub_text(resource),
                    "hint",
                    &scrub_text(hint),
                ));
            }
            Self::Conflict { what, .. } => {
                out.extend_from_slice(&log_one("conflict", "what", &scrub_text(what)));
            }
            Self::UnsupportedMedia { operation, reason } => {
                out.extend_from_slice(&log_two(
                    "unsupported_media",
                    "operation",
                    &scrub_operation(operation),
                    "reason",
                    &scrub_text(reason),
                ));
            }
            Self::CorruptMedia { operation, reason } => {
                out.extend_from_slice(&log_two(
                    "corrupt_media",
                    "operation",
                    &scrub_operation(operation),
                    "reason",
                    &scrub_text(reason),
                ));
            }
            Self::Io {
                operation, path, ..
            } => {
                let redacted = redact_path(path);
                out.extend_from_slice(&log_two(
                    "io",
                    "operation",
                    &scrub_operation(operation),
                    "location",
                    &redacted,
                ));
                if diagnostic == DiagnosticMode::On {
                    write!(out, " path={}", json_field(&path.to_string_lossy()))
                        .expect("write to Vec");
                }
            }
            Self::Storage { what, .. } => {
                out.extend_from_slice(&log_one("storage", "what", &scrub_text(what)));
            }
            Self::Cancelled => out.extend_from_slice(b"error.code=cancelled"),
            Self::InvariantViolation { why } => {
                out.extend_from_slice(&log_one(
                    "invariant_violation",
                    "why",
                    &scrub_operation(why),
                ));
            }
        }
        String::from_utf8(out).unwrap_or_else(|_| "<log encoding error>".to_owned())
    }

    /// Raw path held by an `Io` error, for the caller to act on. Returns
    /// `None` for every other variant and never reaches a log.
    #[must_use]
    pub fn diagnostic_origin(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The stable machine code for this error (used by the IPC boundary).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Validation { .. } => "validation",
            Self::Permission { .. } => "permission",
            Self::Unavailable { .. } => "unavailable",
            Self::Conflict { .. } => "conflict",
            Self::UnsupportedMedia { .. } => "unsupported_media",
            Self::CorruptMedia { .. } => "corrupt_media",
            Self::Io { .. } => "io",
            Self::Storage { .. } => "storage",
            Self::Cancelled => "cancelled",
            Self::InvariantViolation { .. } => "invariant_violation",
        }
    }

    /// Convenience builder for a `Validation` failure with no upstream source.
    #[must_use]
    pub fn validation(
        subject: ValidationSubject,
        field: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::Validation {
            subject,
            field: field.into(),
            reason: reason.into(),
            source: None,
        }
    }

    /// Convenience builder for an `Io` failure.
    #[must_use]
    pub fn io(
        operation: impl Into<String>,
        source: std::io::Error,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self::Io {
            operation: operation.into(),
            source,
            path: path.into(),
        }
    }

    /// Convenience builder for an `Unavailable` failure (transient resource
    /// problem, e.g. revoked permissions or an unmounted root).
    #[must_use]
    pub fn unavailable(resource: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::Unavailable {
            resource: resource.into(),
            hint: hint.into(),
            source: None,
        }
    }

    /// Convenience builder for an `Unavailable` failure observed through a
    /// failing upstream call; the upstream error is kept as `source` per the
    /// "infrastructure errors keep their source" rule.
    pub fn unavailable_with_source<E>(
        resource: impl Into<String>,
        hint: impl Into<String>,
        source: E,
    ) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Unavailable {
            resource: resource.into(),
            hint: hint.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Convenience builder for a `Conflict` failure.
    #[must_use]
    pub fn conflict(what: impl Into<String>) -> Self {
        Self::Conflict {
            what: what.into(),
            source: None,
        }
    }

    /// Convenience builder for a `Permission` failure with a path-free kind.
    #[must_use]
    pub fn permission(operation: impl Into<String>, kind: PermKind) -> Self {
        Self::Permission {
            operation: operation.into(),
            kind,
            source: None,
        }
    }
}

mod privacy;

#[cfg(test)]
#[path = "error/tests.rs"]
mod privacy_tests;

pub(crate) use privacy::{
    aspect, json_field, log_one, log_two, redact_display, scrub_operation, scrub_text,
};
pub use privacy::{PermKind, Subject};

#[cfg(test)]
pub(crate) use privacy::{is_absolute_path_start, path_start_len};
