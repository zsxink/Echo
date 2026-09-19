//! Shared, test-only lexical architecture detectors.
//!
//! The guards deliberately avoid an AST dependency: they make a small set of
//! structural boundaries executable and report the matching source line. This
//! file is path-included only by architecture integration tests; it is not a
//! production Core module.

#![allow(dead_code)] // Each crate guard uses a different subset of shared detectors.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

/// Crates `echo-core` must never reference (platform / player / UI / renderer).
pub const FORBIDDEN_CRATES: &[&str] = &[
    "tauri",
    "tauri-build",
    "tauri-plugin",
    "libmpv",
    "mpv",
    "mpv2",
    "react",
    "flutter",
    "flutter_rust_bridge",
    "wry",
    "global-hotkey",
    "tray-icon",
];

/// Classify a path under `src/` into a Core layer.
#[must_use]
pub fn layer_of(relative: &Path) -> &'static str {
    match relative.components().next() {
        Some(Component::Normal(name)) if name == "domain" => "domain",
        Some(Component::Normal(name)) if name == "application" => "application",
        Some(Component::Normal(name)) if name == "infrastructure" => "infrastructure",
        _ => "other",
    }
}

#[must_use]
pub fn is_rust_source(relative: &Path) -> bool {
    relative
        .extension()
        .is_some_and(|extension| extension == "rs")
}

#[must_use]
pub fn is_exempt_source(relative: &Path) -> bool {
    // Compare on components, not string `/`-separated paths: on Windows the
    // lossy string uses `\`, so `contains("/tests/")` silently fails to
    // exempt `application\import\tests\report.rs` and the arch test starts
    // flagging test-only infrastructure references.
    let components: Vec<_> = relative.components().collect();
    components.iter().any(|component| {
        matches!(component, Component::Normal(name) if {
            let name = name.to_string_lossy();
            name == "tests" || name == "benches"
        })
    }) || components.last().is_some_and(|component| {
        matches!(component, Component::Normal(name) if {
            let name = name.to_string_lossy();
            name == "tests.rs" || name.ends_with("_test.rs") || name.ends_with(".test.rs")
        })
    })
}

/// Returns forbidden dependency declarations in a Cargo manifest.
#[must_use]
pub fn manifest_violations(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with('[')
                || !line.contains('=')
            {
                return None;
            }
            let key = line.split('=').next()?.trim().trim_matches('"');
            FORBIDDEN_CRATES
                .contains(&key)
                .then(|| format!("manifest declares forbidden dependency: {key}"))
        })
        .collect()
}

/// Reports any code-line reference to a forbidden crate.
///
/// Known lexical limits: macro and `include!` expansions are not inspected,
/// and noncanonical spacing such as `crate :: foo` evades literal matching.
/// Line comments are ignored, but string literals are not parsed.
#[must_use]
pub fn crate_use_violations(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let code = code_before_comment(line);
            FORBIDDEN_CRATES.iter().find_map(|crate_name| {
                (code.contains(&format!("{crate_name}::"))
                    || code.contains(&format!("::{crate_name}")))
                .then(|| format!("source references forbidden crate '{crate_name}': {line}"))
            })
        })
        .collect()
}

/// Reports outward Core layer references, including inline qualified paths.
#[must_use]
pub fn layering_violations(layer: &str, source: &str) -> Vec<String> {
    let forbidden: &[&str] = match layer {
        "domain" => &["crate::infrastructure", "crate::application"],
        "application" => &["crate::infrastructure"],
        _ => &[],
    };
    source
        .lines()
        .flat_map(|line| {
            let code = code_before_comment(line);
            forbidden
                .iter()
                .copied()
                .filter(move |edge| code.contains(*edge))
                .map(move |edge| format!("{layer} references {edge}: {line}"))
        })
        .collect()
}

/// Reports platform `cfg` business branches.
#[must_use]
pub fn platform_cfg_violations(source: &str) -> Vec<String> {
    const NEEDLES: &[&str] = &[
        "cfg(target_os",
        "#[cfg(windows)]",
        "#[cfg(unix)]",
        "#[cfg(target_family",
        "cfg!(target_os",
        "cfg!(windows)",
        "#[cfg(not(windows))]",
    ];
    source
        .lines()
        .filter(|line| NEEDLES.iter().any(|needle| line.contains(needle)))
        .map(|line| format!("platform cfg business branch: {line}"))
        .collect()
}

/// Reports `unsafe` usage in non-exempt production code.
#[must_use]
pub fn unsafe_violations(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            (trimmed.starts_with("unsafe") && !trimmed.starts_with("unsafe impl Send"))
                .then(|| format!("unsafe block outside exempt source: {line}"))
        })
        .collect()
}

#[must_use]
pub fn all_core_source_violations(source: &str, layer: &str, exempt: bool) -> Vec<String> {
    let production_source = without_test_modules(source);
    let mut violations = crate_use_violations(&production_source);
    violations.extend(layering_violations(layer, &production_source));
    if !exempt {
        violations.extend(unsafe_violations(&production_source));
    }
    violations.extend(platform_cfg_violations(&production_source));
    violations
}

/// Platform code may not rebuild a song's absolute path from its root.
#[must_use]
pub fn platform_path_violations(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|line| code_before_comment(line).contains("absolute_path().join("))
        .map(|line| format!("platform reconstructs a song absolute path: {line}"))
        .collect()
}

/// Structural signals for playback-domain decisions that belong in Core.
#[must_use]
pub fn platform_playback_violations(source: &str) -> Vec<String> {
    const RULES: &[(&str, &str)] = &[
        ("playback-context page traversal", "const PAGE_SIZE"),
        ("playlist playback order reversal", "songs.reverse()"),
        ("playback availability verdict", "SongAvailability::"),
    ];
    source
        .lines()
        .filter_map(|line| {
            let code = code_before_comment(line);
            RULES.iter().find_map(|(reason, needle)| {
                code.contains(needle)
                    .then(|| format!("platform reimplements {reason}: {line}"))
            })
        })
        .collect()
}

/// No exceptions are approved. An added entry fails rather than normalizing a violation.
pub const PLATFORM_RULE_ALLOWLIST: &[(&str, &str)] = &[];

pub fn assert_platform_allowlist_only_shrinks() {
    assert!(
        PLATFORM_RULE_ALLOWLIST.is_empty(),
        "platform architecture allow-list must only shrink: {PLATFORM_RULE_ALLOWLIST:?}"
    );
}

#[must_use]
pub fn collect_rust_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files
}

fn walk(directory: &Path, root: &Path, files: &mut Vec<(PathBuf, String)>) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            walk(&path, root, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("path under source root")
                .to_path_buf();
            if is_rust_source(&relative) {
                files.push((relative, fs::read_to_string(path).expect("read source")));
            }
        }
    }
}

#[must_use]
pub fn workspace_members(workspace_root: &Path) -> Vec<PathBuf> {
    let manifest =
        fs::read_to_string(workspace_root.join("Cargo.toml")).expect("workspace manifest");
    let mut members = Vec::new();
    let mut in_members = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members = [") {
            in_members = true;
        } else if in_members && trimmed.starts_with(']') {
            break;
        } else if in_members {
            let member = trimmed.trim_end_matches(',').trim_matches('"');
            if !member.is_empty() {
                members.push(workspace_root.join(member));
            }
        }
    }
    members
}

#[must_use]
pub fn deduplicate(violations: Vec<String>) -> BTreeSet<String> {
    violations.into_iter().collect()
}

fn code_before_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _)| code)
}

/// Remove `#[cfg(test)] mod … { … }` blocks before checking production-layer
/// edges. This is deliberately narrow, not a parser: it prevents test fixtures
/// from being mistaken for production dependencies while leaving the detector
/// itself lexical and its remaining limits documented above.
fn without_test_modules(source: &str) -> String {
    let mut output = String::new();
    let mut test_attribute = false;
    let mut test_module_depth: Option<i32> = None;
    for line in source.lines() {
        if let Some(depth) = test_module_depth.as_mut() {
            *depth += brace_delta(line);
            if *depth <= 0 {
                test_module_depth = None;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]" {
            test_attribute = true;
            continue;
        }
        if test_attribute && trimmed.starts_with("mod ") {
            test_module_depth = Some(brace_delta(line));
            test_attribute = false;
            continue;
        }
        test_attribute = false;
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn brace_delta(line: &str) -> i32 {
    let code = code_before_comment(line);
    // Explicit conversion rather than `as`: both counts are per-line character
    // counts, so the invariant holds, and `try_into` turns it into a loud
    // failure instead of a silent wrap if it ever stops holding.
    let opens: i32 = code
        .matches('{')
        .count()
        .try_into()
        .expect("brace count fits");
    let closes: i32 = code
        .matches('}')
        .count()
        .try_into()
        .expect("brace count fits");
    opens - closes
}
