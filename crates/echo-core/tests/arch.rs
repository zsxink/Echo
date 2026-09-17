//! Architecture tests for `echo-core`.
//!
//! Reusable lexical detectors live in Core's dev-only support so desktop and
//! Tauri guards enforce complementary boundaries with the same mechanics.

use std::{fs, path::Path};

#[path = "../src/domain/arch_test_support.rs"]
mod arch_test_support;

use arch_test_support::{
    all_core_source_violations, collect_rust_files, crate_use_violations, deduplicate,
    is_exempt_source, layer_of, layering_violations, manifest_violations, platform_cfg_violations,
    unsafe_violations, workspace_members,
};

fn manifest_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

fn source_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn rejects_forbidden_manifest_dep() {
    assert!(!manifest_violations("[dependencies]\ntauri = \"2\"\nmpv = \"0.9\"\n").is_empty());
    assert!(manifest_violations("[workspace.dependencies]\nserde = \"1\"\n").is_empty());
}

#[test]
fn rejects_forbidden_crate_use_including_inline_qualified_path() {
    assert!(!crate_use_violations("use tauri::Manager;\n").is_empty());
    assert!(!crate_use_violations("fn f() { let _ = libmpv::Client; }\n").is_empty());
    assert!(crate_use_violations("use serde::{Deserialize, Serialize};\n").is_empty());
}

#[test]
fn rejects_upward_layer_edges_including_inline_qualified_path() {
    assert!(
        !layering_violations("domain", "use crate::infrastructure::sqlite::Repo;\n").is_empty()
    );
    assert!(
        !layering_violations("domain", "fn f() { crate::application::ports::X::go(); }\n")
            .is_empty()
    );
    assert!(!layering_violations(
        "application",
        "fn f() { crate::infrastructure::X::go(); }\n"
    )
    .is_empty());
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

#[test]
fn echo_core_complies_with_architecture() {
    let manifest = fs::read_to_string(manifest_path()).expect("core manifest");
    assert!(
        manifest_violations(&manifest).is_empty(),
        "forbidden core manifest dependency"
    );

    let files = collect_rust_files(&source_root());
    assert!(!files.is_empty(), "expected Core sources");
    let mut violations = Vec::new();
    for (relative, source) in files {
        if is_exempt_source(&relative) || relative == Path::new("domain/arch_test_support.rs") {
            continue;
        }
        violations.extend(
            all_core_source_violations(&source, layer_of(&relative), is_exempt_source(&relative))
                .into_iter()
                .map(|violation| format!("{}: {violation}", relative.display())),
        );
    }
    let violations = deduplicate(violations);
    assert!(
        violations.is_empty(),
        "architecture violations:\n{violations:#?}"
    );
}

#[test]
fn every_workspace_crate_has_an_executable_arch_guard() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let missing: Vec<_> = workspace_members(&workspace)
        .into_iter()
        .filter_map(|member| {
            let guard = member.join("tests/arch.rs");
            match fs::read_to_string(&guard) {
                Ok(source) if source.contains("#[test]") => None,
                _ => Some(guard),
            }
        })
        .collect();
    assert!(
        missing.is_empty(),
        "workspace crates without executable architecture guards: {missing:#?}"
    );
}
