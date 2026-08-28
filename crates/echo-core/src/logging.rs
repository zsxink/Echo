//! Logging privacy guard and test-logger integration (task 1.8).
//!
//! Echo's default log output must never contain full absolute paths, lyrics
//! text, tag strings or file contents — only opaque identifiers and hashes.
//! This module provides the small, dependency-light primitives that make that
//! property enforceable:
//!
//! - [`redact_path`] turns an absolute path into a stable short hash plus the
//!   file name, so operators can recognise a file without revealing its
//!   location.
//! - [`DiagnosticMode`] is the single, explicit opt-in that re-enables
//!   desensitized path information. The runtime (echo-desktop) owns the switch;
//!   business code never reads it.
//! - [`init_test_logger`] installs a `tracing` subscriber that captures JSON
//!   lines, used only by tests to assert the privacy policy.
//!
//! See `docs/LOGGING.md` for the full conventions (field names, diagnostics
//! directory, frontend behaviour).

use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing_subscriber::prelude::*;

/// Controls whether desensitized path context may be emitted.
///
/// Default is off: [`Error::to_log`](crate::error::Error::to_log) and
/// [`redact_path`] then produce only the opaque hash + file name. When the
/// desktop runtime explicitly enables `diagnostic` (an opt-in user setting),
/// the source path may be attached to the event as a `path` field — this is
/// the ONLY exception to the default privacy policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiagnosticMode {
    /// Default. Opaque fields only (hashes, IDs, relative paths).
    #[default]
    Off,
    /// Opt-in diagnostic mode: desensitized source paths may be emitted.
    On,
}

/// Stable FNV-1a 64-bit hash over the normalized path bytes.
///
/// Deliberately *not* cryptographically secure and not collision-resistant —
/// it is a log-redaction tag, documented as such. A stable, dependency-light
/// choice: `std::hash::DefaultHasher` is not stable across runs or crates, and
/// echo-core must not pull `blake3`/`sha2` in for a privacy helper.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Normalize a path for hashing so the same path is stable across platforms
/// and separators (both `/` and `\`): strip a single trailing separator and
/// use `/` only.
///
/// Deliberately platform-independent: a hash tag computed from
/// `/Users/someone/Music/song.mp3` on macOS equals the tag computed from
/// `\Users\someone\Music\song.mp3` on Windows for the same logical file.
fn normalized_bytes(path: &Path) -> Vec<u8> {
    let mut source = path.to_string_lossy().into_owned();
    while source.len() > 1 && matches!(source.as_bytes()[source.len() - 1], b'/' | b'\\') {
        source.pop();
    }
    let mut out = Vec::with_capacity(source.len());
    for b in source.bytes() {
        if b == b'\\' {
            out.push(b'/');
        } else {
            out.push(b);
        }
    }
    out
}

/// Normalize free text for hashing: trim and collapse ASCII whitespace so
/// line-ending differences do not change the tag.
fn normalized_value(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut pending_space = false;
    let mut started = false;
    for b in value.bytes() {
        if b.is_ascii_whitespace() {
            pending_space = true;
        } else {
            if started && pending_space {
                out.push(b' ');
            }
            out.push(b);
            pending_space = false;
            started = true;
        }
    }
    out
}

/// Redact an absolute path into a stable short hash plus the file name.
///
/// Default output shape (privacy guard): `song.mp3 (a3b1c2d4)` — location-free.
/// When [`DiagnosticMode`] is `On` the caller may additionally attach the
/// original path in a separate `path` field; business code is never allowed to
/// bypass this and log the raw path directly.
///
/// The hash is FNV-1a over the normalized path bytes and is **not**
/// cryptographically secure; it is sufficient only to correlate log lines about
/// the same file without revealing the filesystem layout.
#[must_use]
pub fn redact_path(path: &Path) -> String {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "<unknown>".to_owned());
    let hash = fnv1a(&normalized_bytes(path));
    format!("{file_name} ({hash:016x})")
}

/// Stable short hash of any sensitive free-text value, with no file name.
///
/// Used for lyrics/tag/file-content material that must be excluded from logs:
/// even in diagnostic mode Echo does not record the text, only an opaque tag
/// that two log lines can compare. Not cryptographically secure.
#[must_use]
pub fn redact_sensitive(value: &str) -> String {
    format!("{:08x}", fnv1a(&normalized_value(value)))
}

/// Returns the display name for the local diagnostics directory.
///
/// Echo settles on `echo/logs` under the platform app-data directory; the
/// desktop runtime owns resolving the actual absolute location (a platform
/// concern, never canonicalized here). Panic hooks and crash dumps go there.
#[must_use]
pub const fn diagnostics_dir_name() -> &'static str {
    "echo/logs"
}

// ---------------------------------------------------------------------------
// Test logger
// ---------------------------------------------------------------------------

/// Install a `tracing` subscriber that captures JSON lines.
///
/// Test-only: intended for tests that assert the privacy property. It records
/// every event as JSON into a bracketed buffer and restores whatever
/// subscriber was installed before when the returned guard is dropped. Global
/// subscriber initialization in production is owned by the runtime
/// (`echo-desktop`, task 7.x) — core never installs a global subscriber.
#[must_use]
pub fn init_test_logger() -> TestLogGuard {
    let buffer = Arc::new(Mutex::new(Vec::<String>::new()));
    let previous = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(tracing_subscriber::fmt::format().json().flatten_event(true))
                .with_writer(BufferWriter(buffer.clone())),
        ),
    );
    TestLogGuard {
        buffer,
        previous: Some(previous),
    }
}

/// Guard returned by [`init_test_logger`]. Dropping it restores the previous
/// subscriber and releases the capture buffer.
#[derive(Debug)]
pub struct TestLogGuard {
    buffer: Arc<Mutex<Vec<String>>>,
    previous: Option<tracing::subscriber::DefaultGuard>,
}

impl TestLogGuard {
    /// Drain the JSON lines captured so far (each is one formatted event).
    ///
    /// # Panics
    ///
    /// Panics if the capture buffer's mutex is poisoned (only possible if a
    /// thread holding it panicked while a subscriber was active).
    #[must_use]
    pub fn drain_json(&self) -> Vec<String> {
        let mut lines = self.buffer.lock().expect("log buffer poisoned");
        std::mem::take(&mut *lines)
    }
}

impl Drop for TestLogGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            drop(previous);
        }
    }
}

/// `MakeWriter` that appends each formatted line to the shared buffer. The
/// writer is cheap to construct per event and never loses state across writes
/// (lines are appended in `write`, so partial writes accumulate correctly).
#[derive(Clone, Debug)]
struct BufferWriter(Arc<Mutex<Vec<String>>>);

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len();
        // Own the buffer so the temporary `Cow` from `from_utf8_lossy` — which
        // carries a `Drop` — can be released before we touch the shared map.
        let s = String::from_utf8_lossy(buf).into_owned();
        {
            let mut lines = self.0.lock().expect("log buffer poisoned");
            for line in s.lines() {
                if !line.trim().is_empty() {
                    lines.push(line.to_owned());
                }
            }
        } // guard dropped here, before returning
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for BufferWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn redact_path_keeps_name_and_hash_and_drops_location() {
        let path = Path::new("/Users/someone/Music/song.mp3");
        let out = redact_path(path);
        assert!(out.contains("song.mp3"), "file name must survive: {out}");
        assert!(!out.contains("Users"), "directory must not survive: {out}");
        assert!(
            !out.contains("someone"),
            "directory must not survive: {out}"
        );
        assert!(!out.contains('/'), "no separator: {out}");
        // Stable across calls.
        assert_eq!(out, redact_path(path));
        // Same visual path on Windows produces the same file-name suffix.
        let win = Path::new(r"C:\Users\someone\Music\song.mp3");
        assert!(redact_path(win).contains("song.mp3"));
        // Same logical path with Windows separators shares one hash tag (the hash is
        // the only thing compared here — `file_name()` of a backslash path is
        // platform-specific, and such paths do not occur as real `Path`s on POSIX).
        let slash = redact_path(Path::new("/Users/someone/Music/song.mp3"));
        let backslash = redact_path(Path::new("\\Users\\someone\\Music\\song.mp3"));
        assert_eq!(
            slash
                .rsplit('(')
                .next()
                .unwrap_or_default()
                .trim_end_matches(')'),
            backslash
                .rsplit('(')
                .next()
                .unwrap_or_default()
                .trim_end_matches(')')
        );
        // Hash fragment shape: 16 lowercase hex digits.
        let tail = out
            .strip_prefix("song.mp3 (")
            .expect("prefix")
            .strip_suffix(')')
            .expect("suffix");
        assert_eq!(tail.len(), 16, "short hash hex length");
        assert!(tail.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn redact_sensitive_is_stable_and_opaque() {
        let a = "Sky full of stars, and I'm the only one\nSomeone else";
        let b = "  Sky   full of stars, and I'm the only one\nSomeone else  ";
        assert_eq!(redact_sensitive(a), redact_sensitive(b));
        assert_eq!(redact_sensitive(a).len(), 16);
        assert!(!redact_sensitive(a).contains("stars"));
        assert!(!redact_sensitive(a).contains("Someone"));
    }
}
