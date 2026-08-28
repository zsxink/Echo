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
    #[error("validation failed on {subject:?} ({field}): {reason}")]
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
    #[error("permission denied: {operation} ({kind})")]
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
    #[error("resource unavailable: {resource}")]
    Unavailable {
        /// What is unavailable (redacted for logs).
        resource: String,
        /// Human-readable explanation for the UI (message key).
        hint: String,
    },

    /// The requested change conflicts with current state (duplicate identity,
    /// concurrent edit, journal claim already taken…). Safe to retry with new
    /// input; never overwrite silently.
    #[error("conflict: {what}")]
    Conflict {
        /// What conflicted (e.g. `playlist name already taken`).
        what: String,
        /// Optional upstream error (e.g. unique-constraint violation).
        source: Option<BoxedSource>,
    },

    /// The media format is outside the supported matrix (not a corruption —
    /// the file may be perfectly fine, just not a supported type).
    #[error("unsupported media: {operation}")]
    UnsupportedMedia {
        /// What was being attempted.
        operation: String,
        /// Why it is unsupported.
        reason: String,
    },

    /// A supported container/stream is damaged or unreadable past recovery.
    #[error("corrupt media: {operation}")]
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
    #[error("storage error: {what}")]
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
    #[error("invariant violation: {why}")]
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
            Self::Unavailable { resource, hint } => {
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
                out.extend_from_slice(&log_one("invariant_violation", "why", why));
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

/// Build a `error.code=<code> <key>=<value>` log fragment.
fn log_one(code: &'static str, key: &'static str, value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    write!(out, "error.code={code} {key}={}", json_field(value)).expect("write to Vec");
    out
}

/// Build a `error.code=<code> k1=v1 k2=v2` log fragment.
fn log_two(code: &'static str, k1: &'static str, v1: &str, k2: &'static str, v2: &str) -> Vec<u8> {
    let mut out = log_one(code, k1, v1);
    write!(out, " {k2}={}", json_field(v2)).expect("write to Vec");
    out
}

/// Display form of a [`PermKind`].
const fn aspect(kind: PermKind) -> &'static str {
    match kind {
        PermKind::Denied => "denied",
        PermKind::ReadOnly => "read_only",
        PermKind::NotOwner => "not_owner",
    }
}

/// The permission category, kept path-free.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermKind {
    /// Access denied (OS permission).
    Denied,
    /// Operation not permitted because the resource is read-only.
    ReadOnly,
    /// The marker/owner check failed (staging dir not owned by Echo).
    NotOwner,
}

impl fmt::Display for PermKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Denied => "denied",
            Self::ReadOnly => "read_only",
            Self::NotOwner => "not_owner",
        })
    }
}

/// Convenience alias for use-site ergonomics.
pub use ValidationSubject as Subject;

// ---------------------------------------------------------------------------
// Scrubbers feeding `to_log` (mirror of the task-1.8 helpers, reused here so
// the classification and the logging policy stay in one place).
// ---------------------------------------------------------------------------

/// Field keys whose values are replaced by an opaque hash when they appear in
/// a free-text operation description.
const SENSITIVE_KV_KEYS: &[&str] = &[
    "title", "artist", "album", "genre", "lyric", "lyrics", "tag", "tags", "content", "payload",
    "path", "paths", "reason",
];

/// Scrub a free-text operation description into a log-safe line.
fn scrub_operation(operation: &str) -> String {
    let step1 = scrub_sensitive_values(operation);
    redact_path_spans(&step1)
}

/// If `rest` begins with a sensitive `<key>=`, return the key and slice after.
fn take_sensitive_key(rest: &str) -> Option<(&'static str, &str)> {
    for key in SENSITIVE_KV_KEYS {
        if let Some(after) = rest.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
            return Some((key, after));
        }
    }
    None
}

/// Return the value (up to the next sensitive `key=` or the end) and the rest.
fn split_kv_value(after_eq: &str) -> (&str, &str) {
    let mut cut = after_eq.len();
    let mut scan = after_eq;
    while !scan.is_empty() {
        if take_sensitive_key(scan).is_some() {
            cut = after_eq.len() - scan.len();
            break;
        }
        let ch = scan.chars().next().unwrap();
        scan = &scan[ch.len_utf8()..];
    }
    (&after_eq[..cut], &after_eq[cut..])
}

/// Replace each sensitive `key=value` field with `key=<opaque hash>`.
fn scrub_sensitive_values(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((key, after_eq)) = take_sensitive_key(rest) {
            out.push_str(key);
            out.push('=');
            let (value, remaining) = split_kv_value(after_eq);
            out.push_str(&redact_sensitive(value));
            rest = remaining;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// Redact bare absolute-path spans (which may contain spaces) not already
/// behind a `key=`. The span terminates at `]`, `)`, `(`, `,` or the end.
fn redact_path_spans(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_absolute_path_start(bytes, i) {
            let start = i;
            i += path_start_len(bytes, i);
            while i < bytes.len() && !matches!(bytes[i], b']' | b')' | b'(' | b',') {
                i += 1;
            }
            let span = &text[start..i];
            out.push_str(&redact_path(Path::new(span)));
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Whether an absolute-path span starts at byte `i`.
fn is_absolute_path_start(bytes: &[u8], i: usize) -> bool {
    match bytes[i] {
        b'/' | b'\\' => true,
        b'f' if bytes[i..].starts_with(b"file://") => true,
        _ => {
            i + 2 < bytes.len()
                && bytes[i].is_ascii_alphabetic()
                && bytes[i + 1] == b':'
                && matches!(bytes[i + 2], b'/' | b'\\')
        }
    }
}

/// Length (in bytes) of the path-start token at `i`.
fn path_start_len(bytes: &[u8], i: usize) -> usize {
    if bytes[i] == b'f' && bytes[i..].starts_with(b"file://") {
        7
    } else {
        1
    }
}

/// Hash free-text value so lyrics/tags/payload never reach a log verbatim.
fn scrub_text(text: &str) -> String {
    redact_sensitive(text)
}

/// Quote a free-text field for consistent JSON-ish log output.
fn json_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
