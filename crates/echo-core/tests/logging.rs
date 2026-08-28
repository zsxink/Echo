//! Logging privacy tests (task 1.8).
//!
//! These assert the core privacy property: default log output never contains
//! full absolute paths, lyrics text, tag strings or file contents — only the
//! safe fields (file name, short hash, operation/error code).

use echo_core::error::Error;
use echo_core::logging::{init_test_logger, DiagnosticMode};
use std::path::PathBuf;

/// An absolute path that must never appear verbatim in default logs.
const ABS_PATH: &str = "/Users/someone/Music/Albums/Night Drive/song.mp3";
/// Lyric-like text that must never appear.
const LYRIC: &str = "I don't know why you don't call me anymore";
/// Tag-like string that must never appear.
const TAG: &str = "VBR MP3 44100Hz 320kbps 2ch 16bit";
/// File-content-like blob that must never appear.
const CONTENT: &str = "ID3\u{3}TAG\x00\x00\x00\x00\x0040% sample of embedded bytes";

fn sample_log_line(mode: DiagnosticMode) -> String {
    let err = Error::Io {
        operation: format!("metadata_read(paths=[{ABS_PATH}]) lyric={LYRIC} tag={TAG}"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"),
        path: PathBuf::from(ABS_PATH),
    };
    err.to_log(mode)
}

#[test]
fn default_log_has_no_absolute_path_no_lyrics_no_tags_no_content() {
    let line = sample_log_line(DiagnosticMode::Off);
    assert!(!line.contains(ABS_PATH), "absolute path leaked: {line}");
    assert!(
        !line.contains("/Users/someone/"),
        "parent dir leaked: {line}"
    );
    assert!(
        !line.contains("Night Drive"),
        "directory segment leaked: {line}"
    );
    assert!(!line.contains("Albums"), "directory segment leaked: {line}");
    assert!(!line.contains(LYRIC), "lyric text leaked: {line}");
    assert!(
        !line.contains("don't call me"),
        "lyric fragment leaked: {line}"
    );
    assert!(!line.contains(TAG), "tag string leaked: {line}");
    assert!(!line.contains("44100Hz"), "tag fragment leaked: {line}");
    assert!(!line.contains(CONTENT), "file content leaked: {line}");
}

#[test]
fn default_log_keeps_safe_fields() {
    let line = sample_log_line(DiagnosticMode::Off);
    assert!(line.contains("error.code=io"), "error code missing: {line}");
    assert!(
        line.contains("operation=") && line.contains("metadata_read"),
        "operation context missing: {line}"
    );
    // File name survives; the location field carries the redacted form.
    assert!(line.contains("song.mp3"), "file name missing: {line}");
    assert!(
        line.contains("location=\"song.mp3 (") || line.contains("location=song.mp3"),
        "redacted location missing: {line}"
    );
}

#[test]
fn captured_test_logger_output_redacts_and_keeps_safe_fields() {
    let guard = init_test_logger();
    let err = Error::Io {
        operation: format!("media_probe title={TAG} lyric={LYRIC} content={CONTENT}"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, "no audio track"),
        path: PathBuf::from(ABS_PATH),
    };
    let line = format!("event=log level=WARN {}", err.to_log(DiagnosticMode::Off));
    tracing::warn!(target: "echo_core::logging", "{}", line);

    let joined = guard.drain_json().join("\n");
    assert!(
        !joined.contains(ABS_PATH),
        "absolute path leaked through tracing: {joined}"
    );
    assert!(
        !joined.contains("/Users/someone/"),
        "parent dir leaked through tracing: {joined}"
    );
    assert!(
        !joined.contains(LYRIC),
        "lyric leaked through tracing: {joined}"
    );
    assert!(
        !joined.contains(TAG),
        "tag leaked through tracing: {joined}"
    );
    assert!(
        !joined.contains(CONTENT),
        "content leaked through tracing: {joined}"
    );
    assert!(
        joined.contains("error.code=io"),
        "error code missing through tracing: {joined}"
    );
    assert!(
        joined.contains("song.mp3"),
        "file name missing through tracing: {joined}"
    );
    drop(guard);
}

#[test]
fn diagnostic_mode_is_the_only_exception_and_adds_full_path() {
    let on = sample_log_line(DiagnosticMode::On);
    assert!(
        on.contains(ABS_PATH),
        "diagnostic mode must reveal path: {on}"
    );
    let off = sample_log_line(DiagnosticMode::Off);
    assert!(!off.contains(ABS_PATH), "default must stay redacted: {off}");
    // The redacted location string is still present in both modes.
    assert!(off.contains("location=\"song.mp3"));
    assert!(on.contains("location=\"song.mp3"));
}

#[test]
fn redact_path_is_stable_and_location_free() {
    use echo_core::redact_path;
    use std::path::Path;

    let p = Path::new(ABS_PATH);
    let a = redact_path(p);
    let b = redact_path(p);
    assert_eq!(a, b, "redaction must be deterministic");
    assert!(a.contains("song.mp3"));
    assert!(!a.contains("Users"));
    assert!(!a.contains('/'));
    assert!(!a.contains('\\'));
    // The hash is a short hex suffix; assert its shape.
    let tail = a
        .strip_prefix("song.mp3 (")
        .expect("prefix")
        .strip_suffix(')')
        .expect("suffix");
    assert_eq!(tail.len(), 16, "short hash hex length");
    assert!(tail.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn redact_sensitive_produces_opaque_hash_for_lyrics_and_content() {
    use echo_core::redact_sensitive;
    let h1 = redact_sensitive(format!("{LYRIC} {TAG} {CONTENT}").as_str());
    let h2 = redact_sensitive(LYRIC);
    assert_eq!(h1.len(), 16);
    assert_eq!(h2.len(), 16);
    assert!(!h1.contains(LYRIC));
    assert!(!h1.contains(TAG));
    assert_eq!(
        redact_sensitive("  business   lyrics\nline"),
        redact_sensitive("business lyrics line")
    );
}

#[test]
fn unavailable_keeps_source_and_never_logs_it_raw() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "root stat failed");
    let err = Error::unavailable_with_source("library root", "根目录暂时不可用", io_err);
    // The infrastructure source is preserved for programmatic handling.
    let source = std::error::Error::source(&err).expect("Unavailable keeps the upstream source");
    assert!(source.to_string().contains("root stat failed"));

    // …but the default log line neither leaks the message nor a path.
    let line = err.to_log(DiagnosticMode::Off);
    assert!(line.contains("error.code=unavailable"), "{line}");
    assert!(!line.contains("root stat failed"), "source leaked: {line}");
    assert!(!line.contains(ABS_PATH), "{line}");
}
