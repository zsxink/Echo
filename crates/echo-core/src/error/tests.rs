use super::*;

const ABS_PATH: &str = "/Users/alice/Music/a.mp3";

#[test]
fn conflict_display_never_leaks_absolute_paths() {
    let err = Error::conflict(format!("target exists: {ABS_PATH}"));
    let text = err.to_string();
    assert!(!text.contains(ABS_PATH), "path leaked in Display: {text}");
    assert!(!text.contains("/Users/alice"), "location leaked: {text}");
    assert!(text.contains("a.mp3"), "file name stays visible: {text}");
    assert_eq!(err.code(), "conflict");
}

#[test]
fn validation_and_unavailable_display_stay_path_free() {
    let validation = Error::validation(
        Subject::Path,
        "RelativeMediaPath",
        format!("rejects {ABS_PATH}"),
    );
    assert!(!validation.to_string().contains(ABS_PATH));

    let unavailable =
        Error::unavailable(format!("root {ABS_PATH}"), format!("unmounted: {ABS_PATH}"));
    let text = unavailable.to_string();
    assert!(!text.contains(ABS_PATH), "path leaked in Display: {text}");
}

#[test]
fn invariant_violation_is_scrubbed_in_display_and_log() {
    let err = Error::InvariantViolation {
        why: format!("two active roots for {ABS_PATH}"),
    };
    let display = err.to_string();
    assert!(!display.contains(ABS_PATH), "path leaked: {display}");
    let log = err.to_log(DiagnosticMode::Off);
    assert!(
        !log.contains(ABS_PATH),
        "path leaked in invariant log: {log}"
    );
    assert!(log.contains("error.code=invariant_violation"), "{log}");
}

#[test]
fn io_display_keeps_redacted_location_only() {
    let err = Error::io(
        "rename",
        std::io::Error::other("cross-device link"),
        ABS_PATH,
    );
    let display = err.to_string();
    assert!(!display.contains(ABS_PATH), "{display}");
    assert!(display.contains("a.mp3 ("), "{display}");
    // The raw path stays reachable for the caller to act on.
    assert_eq!(
        err.diagnostic_origin().map(std::path::Path::new),
        Some(std::path::Path::new(ABS_PATH))
    );
}

#[test]
fn to_log_covers_every_variant_and_never_leaks_sensitive_text() {
    // The log line for every variant: stable field keys, path-free and
    // free-text fields scrubbed (hashed), never a raw span of the message.
    let cases: Vec<(Error, &str, &str)> = vec![
        (
            Error::validation(Subject::Id, "SongId", "malformed uuid"),
            "error.code=validation",
            "field=",
        ),
        (
            Error::permission("open root", PermKind::Denied),
            "error.code=permission",
            "kind=",
        ),
        (
            Error::unavailable("library", "root unmounted"),
            "error.code=unavailable",
            "hint=",
        ),
        (
            Error::conflict("playlist name already taken"),
            "error.code=conflict",
            "what=",
        ),
        (
            Error::UnsupportedMedia {
                operation: "probe".to_owned(),
                reason: "wma container".to_owned(),
            },
            "error.code=unsupported_media",
            "reason=",
        ),
        (
            Error::CorruptMedia {
                operation: "tag read".to_owned(),
                reason: "truncated stream".to_owned(),
            },
            "error.code=corrupt_media",
            "reason=",
        ),
        (
            Error::io("rename", std::io::Error::other("cross-device"), ABS_PATH),
            "error.code=io",
            "location=",
        ),
        (
            Error::Storage {
                what: "migration".to_owned(),
                source: std::io::Error::other("disk full").into(),
            },
            "error.code=storage",
            "what=",
        ),
        (Error::Cancelled, "error.code=cancelled", ""),
        (
            Error::InvariantViolation {
                why: "two active roots".to_owned(),
            },
            "error.code=invariant_violation",
            "why=",
        ),
    ];
    for (error, code, key) in cases {
        let log = error.to_log(DiagnosticMode::Off);
        assert!(log.starts_with(code), "{log:?}");
        if !key.is_empty() {
            assert!(log.contains(key), "{log:?}");
        }
        assert!(
            !log.contains(ABS_PATH),
            "a log line never carries an absolute path: {log}"
        );
    }

    // Diagnostics `On` opts into the raw path for `Io` only.
    let io = Error::io("rename", std::io::Error::other("x"), ABS_PATH);
    assert!(io.to_log(DiagnosticMode::On).contains(ABS_PATH));
    // Free-text reasons are hashed, never echoed verbatim.
    let secret = Error::unavailable("library", "lyric text 晴天 never echoes");
    let log = secret.to_log(DiagnosticMode::On);
    assert!(!log.contains("晴天"), "free text is scrubbed: {log}");
    assert!(log.contains("hint="), "{log}");
}

#[test]
fn codes_are_stable_and_permission_aspects_render() {
    for (error, code) in [
        (
            Error::UnsupportedMedia {
                operation: "probe".to_owned(),
                reason: "wma".to_owned(),
            },
            "unsupported_media",
        ),
        (
            Error::CorruptMedia {
                operation: "probe".to_owned(),
                reason: "truncated".to_owned(),
            },
            "corrupt_media",
        ),
        (
            Error::InvariantViolation {
                why: "one active root".to_owned(),
            },
            "invariant_violation",
        ),
        (Error::Cancelled, "cancelled"),
    ] {
        assert_eq!(error.code(), code);
    }
    for (kind, text) in [
        (PermKind::Denied, "denied"),
        (PermKind::ReadOnly, "read_only"),
        (PermKind::NotOwner, "not_owner"),
    ] {
        assert_eq!(aspect(kind), text);
        assert_eq!(kind.to_string(), text);
    }
}

#[test]
fn windows_and_file_protocol_paths_are_redacted_like_the_rest() {
    // The path-span *detection* recognizes drive-letter and `file://`
    // starts on every platform (the byte prefixes are OS-independent).
    assert!(is_absolute_path_start(br"C:\Users\Alice\Music\a.mp3", 0));
    assert!(is_absolute_path_start(
        format!("file://{ABS_PATH}").as_bytes(),
        0
    ));
    assert_eq!(
        path_start_len(format!("file://{ABS_PATH}").as_bytes(), 0),
        7
    );

    // A `file://` URI is replaced by redact_path of the local path behind
    // it — the scheme and the absolute dirs never survive Display, only
    // the `file-name (hash)` form.
    let file_url = format!("unavailable: file://{ABS_PATH}");
    let unavailable = Error::unavailable(file_url, "probe");
    let display = unavailable.to_string();
    assert!(!display.contains(ABS_PATH), "file path leaked: {display}");
    assert!(!display.contains("file://"), "scheme leaked: {display}");
    assert!(
        display.contains("a.mp3 ("),
        "file name stays visible: {display}"
    );
    // The token before the span is preserved.
    assert!(display.starts_with("resource unavailable"), "{display}");
}

#[test]
fn display_of_permission_and_unsupported_media_scrub_paths() {
    let permission = Error::permission(format!("stage trash for {ABS_PATH}"), PermKind::NotOwner);
    let display = permission.to_string();
    assert!(!display.contains(ABS_PATH), "{display}");

    let unsupported = Error::UnsupportedMedia {
        operation: format!("copy from {ABS_PATH}"),
        reason: format!("rejected {ABS_PATH}"),
    };
    let display = unsupported.to_string();
    assert!(!display.contains(ABS_PATH), "{display}");
    assert_eq!(unsupported.code(), "unsupported_media");

    // A source-bearing unavailable error keeps the upstream error
    // reachable (as the enum's `source` field) and its Display path-free.
    let unavailable = Error::unavailable_with_source(
        "library root",
        format!("unmounted {ABS_PATH}"),
        std::io::Error::other("no such device"),
    );
    assert!(
        match &unavailable {
            Error::Unavailable { source, .. } => source.is_some(),
            _ => false,
        },
        "the upstream error is kept as the source"
    );
    assert!(!unavailable.to_string().contains(ABS_PATH));
}
