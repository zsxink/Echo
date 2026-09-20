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
        let urls: Vec<tauri::Url> =
            std::iter::once("file:///Users/x/Music/We%20Will%20Rock%20You%20-%20Queen.flac")
                .map(|s| s.parse().expect("valid url"))
                .collect();
        let paths = open_targets(&urls);
        assert_eq!(paths.len(), 1);
        // The exact spelling differs per host (`/Users/…` vs `\Users\…`), so
        // assert the cross-platform semantics: the path stays absolute, the
        // percent-encoding is decoded, and order is preserved.
        assert!(paths[0].is_absolute(), "decoded path stays absolute");
        assert_eq!(
            paths[0].file_name().and_then(|n| n.to_str()),
            Some("We Will Rock You - Queen.flac"),
            "percent-encoding is decoded on every host",
        );
        assert!(
            !paths[0].to_string_lossy().contains("%20"),
            "no percent-encoding survives the decode",
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
        let urls: Vec<tauri::Url> = [
            "file:///a/One%20Two.flac",
            "http://x/a.flac",
            "file:///b/%E4%B8%AD%E6%96%87.flac",
        ]
        .iter()
        .map(|s| s.parse().expect("valid url"))
        .collect();
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
    }
}
