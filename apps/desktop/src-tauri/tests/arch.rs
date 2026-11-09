//! Architecture guard for the thin Tauri command shell.

#[path = "../../../../crates/echo-core/src/domain/arch_test_support.rs"]
mod arch_test_support;

use std::path::Path;

use arch_test_support::{
    assert_platform_allowlist_only_shrinks, collect_rust_files, deduplicate,
    platform_path_violations, platform_playback_violations,
};

#[test]
fn rejects_synthetic_platform_domain_rule_violations() {
    assert!(
        !platform_path_violations("root.absolute_path().join(song.path().normalized())\n")
            .is_empty()
    );
    assert!(!platform_playback_violations("songs.reverse();\n").is_empty());
    assert!(platform_playback_violations(
        "#[cfg(test)]\nmod fixtures {\nassert_eq!(state, SongAvailability::Available);\n}\n"
    )
    .is_empty());
}

#[test]
fn tauri_shell_does_not_reimplement_core_playback_rules() {
    assert_platform_allowlist_only_shrinks();
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    for (relative, source) in collect_rust_files(&source_root) {
        violations.extend(
            platform_path_violations(&source)
                .into_iter()
                .chain(platform_playback_violations(&source))
                .map(|violation| format!("{}: {violation}", relative.display())),
        );
    }
    let violations = deduplicate(violations);
    assert!(
        violations.is_empty(),
        "Tauri architecture violations:\n{violations:#?}"
    );
}

/// The macOS notch player is a *companion window*, not a second player: it must
/// never construct a playback session of its own (spec `desktop-app-shell`:
/// 伴随窗口不构成第二个实例). It only asks the shell to resize itself; every
/// transport command goes through the shared IPC surface.
#[test]
fn notch_window_never_constructs_a_second_playback_session() {
    let notch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("notch");
    let mut violations = Vec::new();
    for (relative, source) in collect_rust_files(&notch) {
        for needle in [
            "PlayerController",
            "PlaybackCoordinator",
            "Queue::new",
            "player::port::PlayerCommand",
        ] {
            if source.contains(needle) {
                violations.push(format!("{}: {needle}", relative.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "the notch window must not own a playback session:\n{violations:#?}"
    );
}
