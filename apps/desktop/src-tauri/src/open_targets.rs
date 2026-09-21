//! The shell's OS file-open URL→path normalization
//! (normalize-os-file-open-paths).
//!
//! The OS delivers `RunEvent::Opened` payloads as `file://` URL strings with
//! percent-encoding; `to_file_path()` validates the scheme and decodes the
//! encoding exactly once, here — the single owner of the URL→path boundary.
//! Anything that is not a local file (`http://`, `smb://`, …) is dropped with a
//! warning and must never reach the playback chain.
//!
//! Pure and platform-independent so the conversion semantics stay unit-testable
//! on every OS, even though the only caller (`RunEvent::Opened` in `main.rs`)
//! is macOS-only. Kept in its own module because `main.rs` sits against the
//! 1000-line scale ceiling.

use std::path::PathBuf;

/// Normalize one batch of OS file-open URLs into local filesystem paths.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn open_targets(urls: &[tauri::Url]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| {
            url.to_file_path().map_or_else(
                |()| {
                    eprintln!("open: ignoring non-file open request {url}");
                    None
                },
                Some,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::open_targets;

    #[test]
    fn decodes_percent_encoding_and_keeps_order() {
        // Build the file URL from a real absolute path on the host. A
        // constructed fixture like `/Music/…` is not enough: on Windows a
        // root-only path (`\Music\…`) is not an absolute path with a drive
        // prefix, so `Url::from_file_path` — which requires one — would fail
        // to build the fixture in the first place. `temp_dir()` is absolute
        // on every host.
        let abs = std::env::temp_dir()
            .join("Music")
            .join("We Will Rock You - Queen.flac");
        let source = tauri::Url::from_file_path(&abs).expect("absolute path becomes a file url");
        let urls = vec![source];
        let paths = open_targets(&urls);
        assert_eq!(paths.len(), 1);
        // The exact spelling differs per host (`/var/…` vs `C:\…`), so assert
        // the cross-platform semantics: the path stays absolute, the encoding
        // round-trips to the same leaf name, and order is preserved.
        assert!(paths[0].is_absolute(), "decoded path stays absolute");
        assert_eq!(
            paths[0].file_name().and_then(|n| n.to_str()),
            Some("We Will Rock You - Queen.flac"),
            "space survives the URL round-trip on every host",
        );
        assert_eq!(
            paths[0], abs,
            "the path round-trips exactly on the host that produced it",
        );
    }

    #[test]
    fn drops_non_file_schemes() {
        let urls: Vec<tauri::Url> = ["http://x/a.flac", "smb://host/share/a.flac"]
            .iter()
            .map(|s| s.parse().expect("valid url"))
            .collect();
        assert!(open_targets(&urls).is_empty());
    }

    #[test]
    fn mixed_input_keeps_only_file_targets_in_order() {
        // Same portability note as the test above: build the two file URLs
        // from real absolute paths so the fixture is valid on every host
        // (drive-letter prefix required by `Url::from_file_path` on Windows).
        let first = std::env::temp_dir().join("a").join("One Two.flac");
        let second = std::env::temp_dir().join("b").join("中文.flac");
        let urls = vec![
            tauri::Url::from_file_path(&first).expect("absolute path becomes a file url"),
            "http://x/a.flac".parse().expect("valid url"),
            tauri::Url::from_file_path(&second).expect("absolute path becomes a file url"),
        ];
        let paths = open_targets(&urls);
        assert_eq!(paths.len(), 2, "non-file schemes are dropped");
        // Decoding, ordering and file-only filtering — asserted via the leaf,
        // whose spelling is identical on every host.
        let leaves: Vec<&str> = paths
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(leaves, vec!["One Two.flac", "中文.flac"]);
        assert!(paths.iter().all(|p| p.is_absolute()));
        assert_eq!(paths[0], first);
        assert_eq!(paths[1], second);
    }
}
