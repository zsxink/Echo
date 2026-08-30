//! Writes the generated read-only TypeScript IPC types to
//! `apps/desktop/src/ipc/ipc-types.generated.ts`.
//!
//! CI runs `cargo run -p echo-desktop --bin echo-generate-ipc`, then
//! `git diff --exit-code` fails if the committed file drifted from the Rust
//! DTOs. The frontend imports the generated types through a stable path.

use std::fs;
use std::path::PathBuf;

use echo_desktop::ipc::generated_typescript;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // workspace root: crates/echo-desktop -> apps/desktop/src/ipc
    let out = root
        .parent()
        .expect("crate lives in crates/")
        .parent()
        .expect("workspace root")
        .join("apps/desktop/src/ipc/ipc-types.generated.ts");
    fs::create_dir_all(out.parent().expect("ipc dir")).expect("create ipc dir");
    fs::write(&out, generated_typescript()).expect("write generated types");
    println!("wrote {}", out.display());
}
