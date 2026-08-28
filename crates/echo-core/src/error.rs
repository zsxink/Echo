//! Core error classification (task 2.5, partially scoped for task 1.8).
//!
//! Task 1.8 establishes the logging privacy convention, so the placeholder
//! `Error` type gains a structured [`Error::to_log`] that never embeds raw
//! paths, lyric text, tag strings or file contents. The full variant
//! classification (`Validation / Permission / Unavailable / Conflict /
//! UnsupportedMedia / CorruptMedia / Io / Storage / Cancelled /
//! InvariantViolation`) is expanded by task 2.5.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::logging::{redact_path, redact_sensitive, DiagnosticMode};

/// Top-level error type for the Echo core.
///
/// The variant set is expanded by task 2.5 into the full classification:
/// `Validation / Permission / Unavailable / Conflict / UnsupportedMedia /
/// CorruptMedia / Io / Storage / Cancelled / InvariantViolation`.
#[derive(Debug, Error)]
pub enum Error {
    /// A placeholder variant; replaced by the task 2.5 classification.
    #[error("not yet implemented")]
    NotImplemented,

    /// I/O failure carrying the absolute path that failed. The path is kept
    /// internally so the caller can act on it, but it must never reach a log
    /// line (see [`Error::to_log`]).
    #[error("i/o error on {path}")]
    Io {
        /// The operation that failed.
        operation: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
        /// Absolute path the operation was working on. Never logged raw.
        path: PathBuf,
    },

    /// A corrupt container / unreadable media file.
    #[error("unsupported or corrupt media: {operation}")]
    UnsupportedMedia {
        /// What was being attempted when the media was rejected.
        operation: String,
        /// Absolute path of the media. Never logged raw.
        path: PathBuf,
        /// Reason the media was rejected.
        reason: String,
    },
}

impl Error {
    /// Produce a single structured, path-free log line for this error.
    ///
    /// The default (privacy) behaviour emits only the error code, an operation
    /// context string and a redacted path *hash + file name*. When
    /// `diagnostic == DiagnosticMode::On` a desensitized `path` field is added
    /// — that is the ONLY exception to the default privacy policy, and the
    /// caller (the desktop runtime) is responsible for having cleared the
    /// opt-in.
    ///
    /// Lyrics text, tag strings, file contents and absolute paths are never
    /// included in any mode: the operation string is scrubbed, and the `path`
    /// field appears exclusively through `redact_path`.
    #[must_use]
    pub fn to_log(&self, diagnostic: DiagnosticMode) -> String {
        let mut out = Vec::with_capacity(64);
        match self {
            Self::Io {
                operation, path, ..
            } => {
                let code = "io";
                let redacted = redact_path(path);
                if diagnostic == DiagnosticMode::On {
                    write!(
                        out,
                        "error.code={code} operation={} location={} path={}",
                        json_field(&scrub_operation(operation)),
                        json_field(&redacted),
                        json_field(&path.to_string_lossy())
                    )
                    .expect("write to Vec");
                } else {
                    write!(
                        out,
                        "error.code={code} operation={} location={}",
                        json_field(&scrub_operation(operation)),
                        json_field(&redacted)
                    )
                    .expect("write to Vec");
                }
            }
            Self::UnsupportedMedia {
                operation,
                path,
                reason,
                ..
            } => {
                let code = "unsupported_media";
                let redacted = redact_path(path);
                let scrubbed_reason = scrub_text(reason);
                if diagnostic == DiagnosticMode::On {
                    write!(
                        out,
                        "error.code={code} operation={} reason={} location={} path={}",
                        json_field(&scrub_operation(operation)),
                        json_field(&scrubbed_reason),
                        json_field(&redacted),
                        json_field(&path.to_string_lossy())
                    )
                    .expect("write to Vec");
                } else {
                    write!(
                        out,
                        "error.code={code} operation={} reason={} location={}",
                        json_field(&scrub_operation(operation)),
                        json_field(&scrubbed_reason),
                        json_field(&redacted)
                    )
                    .expect("write to Vec");
                }
            }
            Self::NotImplemented => return "error.code=not_implemented".to_owned(),
        }
        // `write!` into a Vec cannot fail; produce the final string.
        String::from_utf8(out).unwrap_or_else(|_| "<log encoding error>".to_owned())
    }
}

/// Field keys whose values are replaced by an opaque hash when they appear in a
/// free-text operation description. These carry the very material logs must
/// never contain verbatim: file paths, lyric/tag text, media payloads.
const SENSITIVE_KV_KEYS: &[&str] = &[
    "title", "artist", "album", "genre", "lyric", "lyrics", "tag", "tags", "content", "payload",
    "path", "paths", "reason",
];

/// Scrub a free-text operation description into a log-safe line.
///
/// Two defensive passes, in order:
///   1. hash the value of every sensitive `key=value` field (so even a caller
///      that inlined `lyric=…` / `tag=…` / `content=…` / `paths=[…]` cannot leak
///      the material through `Error::to_log`);
///   2. redact any surviving absolute-path span (which may contain spaces) to
///      the `file-name (short-hash)` form.
fn scrub_operation(operation: &str) -> String {
    let step1 = scrub_sensitive_values(operation);
    redact_path_spans(&step1)
}

/// If `rest` begins with a sensitive `<key>=`, return the key and the slice
/// after `=`.
fn take_sensitive_key(rest: &str) -> Option<(&'static str, &str)> {
    for key in SENSITIVE_KV_KEYS {
        if let Some(after) = rest.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
            return Some((key, after));
        }
    }
    None
}

/// Return the value (up to the next sensitive `key=` or the end) and whatever
/// remains after it.
fn split_kv_value(after_eq: &str) -> (&str, &str) {
    let mut cut = after_eq.len();
    let mut scan = after_eq;
    while !scan.is_empty() {
        if let Some((_, _)) = take_sensitive_key(scan) {
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

/// Redact bare absolute-path spans (which may contain spaces) that are not
/// already behind a `key=`. The span terminates at `]`, `)`, `(`, `,` or the
/// end of the string, so a path like `/Users/…/Night Drive/song.mp3` is kept
/// whole and replaced by its `file-name (short-hash)` form.
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

/// Whether an absolute-path span starts at byte `i`: a `/`, a `\`, a
/// `file://` URL, or a drive letter (`C:\` / `C:/`).
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

/// Scrub lyric/tag/file-content material that could have been inlined in a
/// field value (`reason`, …): the text is replaced by an opaque hash.
fn scrub_text(text: &str) -> String {
    redact_sensitive(text)
}

/// Quote a free-text field the same way the JSON sink does, so `to_log` output
/// is consistent whether a test captures it directly or through
/// [`crate::logging::init_test_logger`].
fn json_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => {
                out.push('\\');
                out.push('"');
            }
            '\\' => {
                out.push('\\');
                out.push('\\');
            }
            '\n' => {
                out.push_str("\\n");
            }
            '\r' => {
                out.push_str("\\r");
            }
            '\t' => {
                out.push_str("\\t");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
