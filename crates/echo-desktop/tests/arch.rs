//! Architecture guard for the desktop platform adapter.

use std::path::Path;

#[path = "../../echo-core/src/domain/arch_test_support.rs"]
mod arch_test_support;

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
    assert!(!platform_playback_violations("const PAGE_SIZE: usize = 500;\n").is_empty());
    assert!(!platform_playback_violations("songs.reverse();\n").is_empty());
    assert!(!platform_playback_violations(
        "match song.availability() { SongAvailability::Missing => {} }\n"
    )
    .is_empty());
}

#[test]
fn desktop_adapts_but_does_not_reimplement_playback_domain_rules() {
    assert_platform_allowlist_only_shrinks();
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    for (relative, source) in collect_rust_files(&source_root) {
        violations.extend(
            platform_path_violations(&source)
                .into_iter()
                .map(|violation| format!("{}: {violation}", relative.display())),
        );
        if relative == Path::new("runtime/services.rs") {
            violations.extend(
                platform_playback_violations(&source)
                    .into_iter()
                    .map(|violation| format!("{}: {violation}", relative.display())),
            );
        }
    }
    let violations = deduplicate(violations);
    assert!(
        violations.is_empty(),
        "desktop architecture violations:\n{violations:#?}"
    );
}
