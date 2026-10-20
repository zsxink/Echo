//! Architecture guard for LAN adapters: dependencies point inward to Core.

use std::fs;
use std::path::Path;

#[test]
fn sync_adapter_depends_on_core_without_leaking_transport_into_core() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sync_manifest = fs::read_to_string(workspace.join("crates/echo-sync/Cargo.toml"))
        .expect("read echo-sync manifest");
    let core_manifest = fs::read_to_string(workspace.join("crates/echo-core/Cargo.toml"))
        .expect("read echo-core manifest");

    assert!(sync_manifest.contains("echo-core = { path = \"../echo-core\" }"));
    assert!(!core_manifest.contains("echo-sync"));
    for transport in ["axum", "reqwest", "rustls", "tokio", "mdns-sd"] {
        assert!(!core_manifest.contains(transport));
    }
}
