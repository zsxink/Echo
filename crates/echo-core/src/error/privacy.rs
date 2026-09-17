//! Path/value scrubbing shared by [`Error`](super::Error)'s `Display` and
//! `to_log` output: everything here exists to keep absolute paths and free-text
//! values out of anything that can reach a UI payload, IPC response or log.
//!
//! Nothing here is crate-internal data plumbing — it is a group of pure
//! functions over `&str`/`&[u8]`, which is why it can live outside the enum.

use super::*;

pub(crate) fn log_one(code: &'static str, key: &'static str, value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    write!(out, "error.code={code} {key}={}", json_field(value)).expect("write to Vec");
    out
}

/// Build a `error.code=<code> k1=v1 k2=v2` log fragment.
pub(crate) fn log_two(
    code: &'static str,
    k1: &'static str,
    v1: &str,
    k2: &'static str,
    v2: &str,
) -> Vec<u8> {
    let mut out = log_one(code, k1, v1);
    write!(out, " {k2}={}", json_field(v2)).expect("write to Vec");
    out
}

/// Redact user-facing `Display` text for the free-text error fields: bare
/// absolute-path spans become the `file-name (hash)` form, so no public error
/// message (UI, IPC payload, panic text) ever carries a filesystem location.
/// Unlike the log scrubbers this keeps readable text — only path spans are
/// replaced.
pub(crate) fn redact_display(text: &str) -> String {
    redact_path_spans(text)
}

/// Display form of a [`PermKind`].
pub(crate) const fn aspect(kind: PermKind) -> &'static str {
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
pub use super::ValidationSubject as Subject;

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
pub(crate) fn scrub_operation(operation: &str) -> String {
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
pub(crate) fn scrub_sensitive_values(text: &str) -> String {
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
pub(crate) fn redact_path_spans(text: &str) -> String {
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
pub(crate) fn is_absolute_path_start(bytes: &[u8], i: usize) -> bool {
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
pub(crate) fn path_start_len(bytes: &[u8], i: usize) -> usize {
    if bytes[i] == b'f' && bytes[i..].starts_with(b"file://") {
        7
    } else {
        1
    }
}

/// Hash free-text value so lyrics/tags/payload never reach a log verbatim.
pub(crate) fn scrub_text(text: &str) -> String {
    redact_sensitive(text)
}

/// Quote a free-text field for consistent JSON-ish log output.
pub(crate) fn json_field(s: &str) -> String {
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
