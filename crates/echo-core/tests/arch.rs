//! Architecture tests for `echo-core`.
//!
//! These enforce `CODE_STANDARDS` §3 layering and dependency direction:
//!
//! - `echo-core` must not depend on Tauri, mpv, React or other platform/UI
//!   crates (declared deps and `use` of such crates are both rejected).
//! - `domain` must not import `application` or `infrastructure`; `application`
//!   must not import `infrastructure`.
//! - `echo-core` must not carry platform `cfg` business branches (`cfg(target_os)`
//!   etc.) that would make Core OS-specific.
//!
//! Each detector is a pure function so the "can reject" behaviour is itself
//! unit-tested on synthetic violating/clean inputs, then applied to the real
//! source tree.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

const MANIFEST: &str = "Cargo.toml";

/// Crates `echo-core` must never reference (platform / player / UI / renderer).
const FORBIDDEN_CRATES: &[&str] = &[
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

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST)
}

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Classify a path under `src/` into a layer. Anything not under a known layer
/// gets the empty layer (never a dependency of anything, no outgoing checks).
fn layer_of(rel: &Path) -> &'static str {
    match rel.components().next() {
        Some(Component::Normal(s)) if s == "domain" => "domain",
        Some(Component::Normal(s)) if s == "application" => "application",
        Some(Component::Normal(s)) if s == "infrastructure" => "infrastructure",
        _ => "other",
    }
}

fn is_rust_source(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e == "rs")
}

/// Sources that are allowed to carry platform-conditional code and unsafe
/// blocks: tests, benches and build-time files are not business code.
fn is_exempt_source(rel: &Path) -> bool {
    let s = rel.to_string_lossy();
    s.contains("/tests/")
        || s.contains("/benches/")
        || s.ends_with("_test.rs")
        || s.ends_with(".test.rs")
}

/// Returns human-readable violations for a declared manifest dependency set.
///
/// Matches `name = "…"`, `name = { … }` and `"name" = "…"` dependency keys
/// whose name is a forbidden crate. Section headers like `[dependencies]` have
/// no `=` and are ignored.
fn manifest_violations(manifest: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('[') || !t.contains('=') {
            continue;
        }
        let key = t.split('=').next().unwrap_or("").trim().trim_matches('"');
        if FORBIDDEN_CRATES.contains(&key) {
            v.push(format!("manifest declares forbidden dependency: {key}"));
        }
    }
    v
}

/// Reports `use`/`extern crate` references to forbidden crates in a source body.
fn crate_use_violations(src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("use ") || t.starts_with("pub use ") || t.starts_with("extern crate ") {
            for c in FORBIDDEN_CRATES {
                if line.contains(&format!("{c}::")) || line.contains(&format!("::{c}")) {
                    v.push(format!("source references forbidden crate '{c}': {line}"));
                }
            }
        }
    }
    v
}

/// Reports cross-layer `use crate::…` edges that point outward.
fn layering_violations(layer: &str, src: &str) -> Vec<String> {
    let forbidden: &[&str] = match layer {
        "domain" => &["crate::infrastructure", "crate::application"],
        "application" => &["crate::infrastructure"],
        _ => &[],
    };
    let mut v = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("use ") || t.starts_with("pub use ") {
            for edge in forbidden {
                if line.contains(edge) {
                    v.push(format!("{layer} imports {edge}: {line}"));
                }
            }
        }
    }
    v
}

/// Reports platform `cfg` business branches.
fn platform_cfg_violations(src: &str) -> Vec<String> {
    let needles = [
        "cfg(target_os",
        "#[cfg(windows)]",
        "#[cfg(unix)]",
        "#[cfg(target_family",
        "cfg!(target_os",
        "cfg!(windows)",
        "#[cfg(not(windows))]",
    ];
    let mut v = Vec::new();
    for line in src.lines() {
        if needles.iter().any(|n| line.contains(n)) {
            v.push(format!("platform cfg business branch: {line}"));
        }
    }
    v
}

/// Reports `unsafe` usage in non-exempt production code.
fn unsafe_violations(src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("unsafe") && !t.starts_with("unsafe impl Send") {
            v.push(format!("unsafe block outside exempt source: {line}"));
        }
    }
    v
}

fn all_source_violations(src: &str, layer: &str, exempt: bool) -> Vec<String> {
    let mut v = Vec::new();
    v.extend(crate_use_violations(src));
    v.extend(layering_violations(layer, src));
    if !exempt {
        v.extend(unsafe_violations(src));
    }
    // Platform cfg is forbidden even in tests for the business core; the Core
    // must stay cross-platform. (Kept uniform so a test can't legitimise a
    // cfg-gated product branch.)
    v.extend(platform_cfg_violations(src));
    v
}

// ---------------------------------------------------------------------------
// "Can reject" unit tests: synthetic inputs must be flagged.
// ---------------------------------------------------------------------------

#[test]
fn rejects_forbidden_manifest_dep() {
    let bad = "[dependencies]\ntauri = \"2\"\nmpv = \"0.9\"\n";
    assert!(
        !manifest_violations(bad).is_empty(),
        "tauri/mpv must be rejected"
    );
    let ok = "[workspace.dependencies]\nserde = \"1\"\n";
    assert!(
        manifest_violations(ok).is_empty(),
        "clean manifest must pass"
    );
}

#[test]
fn rejects_forbidden_crate_use() {
    assert!(!crate_use_violations("use tauri::Manager;\n").is_empty());
    assert!(!crate_use_violations("use libmpv::Client;\n").is_empty());
    assert!(crate_use_violations("use serde::{Serialize, Deserialize};\n").is_empty());
}

#[test]
fn rejects_upward_layer_edges() {
    assert!(
        !layering_violations("domain", "use crate::infrastructure::sqlite::Repo;\n").is_empty()
    );
    assert!(!layering_violations("domain", "use crate::application::ports::X;\n").is_empty());
    assert!(!layering_violations("application", "use crate::infrastructure::X;\n").is_empty());
    assert!(layering_violations("application", "use crate::domain::X;\n").is_empty());
    assert!(layering_violations("domain", "use crate::domain::ids::SongId;\n").is_empty());
}

#[test]
fn rejects_platform_cfg_branch() {
    assert!(!platform_cfg_violations("#[cfg(target_os = \"windows\")]\nfn f() {}\n").is_empty());
    assert!(platform_cfg_violations("fn f() -> u8 { 1 }\n").is_empty());
}

#[test]
fn rejects_unsafe_in_production() {
    assert!(!unsafe_violations("unsafe { foo(); }\n").is_empty());
}

// ---------------------------------------------------------------------------
// Real-source compliance tests.
// ---------------------------------------------------------------------------

fn collect_rust_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, String)>) {
    for e in fs::read_dir(dir).expect("read_dir") {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            walk(&p, root, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if is_rust_source(&rel) {
                let text = fs::read_to_string(&p).unwrap();
                out.push((rel, text));
            }
        }
    }
}

#[test]
fn echo_core_complies_with_architecture() {
    let manifest = fs::read_to_string(manifest_path()).unwrap();
    let mviol = manifest_violations(&manifest);
    assert!(mviol.is_empty(), "manifest violations:\n{mviol:#?}");

    let files = collect_rust_files(&source_root());
    assert!(!files.is_empty(), "expected to find core sources");
    let mut all: Vec<String> = Vec::new();
    for (rel, src) in &files {
        let layer = layer_of(rel);
        let exempt = is_exempt_source(rel);
        all.extend(
            all_source_violations(src, layer, exempt)
                .into_iter()
                .map(|v| format!("{}: {v}", rel.display())),
        );
    }
    let dedup: BTreeSet<String> = all.into_iter().collect();
    assert!(
        dedup.is_empty(),
        "architecture violations:\n{}",
        dedup.iter().fold(String::new(), |mut a, b| {
            a.push_str(b);
            a.push('\n');
            a
        })
    );
}
