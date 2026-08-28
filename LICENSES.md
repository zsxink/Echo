# Echo — Third-Party Notices & License Inventory

This file is the human-readable license inventory for Echo's dependency graph,
as of the current lockfiles (`Cargo.lock` and `pnpm-lock.yaml`). It is
generated from `cargo metadata` and the pnpm lockfile and is kept as a
checkpoint artifact: `scripts/verify/checks/task-1.7.mjs` asserts this file
exists and is non-empty, and `cargo deny check` enforces the license policy
for the Rust graph continuously. After lockfile changes that alter the crate
set, regenerate this file (see footer) and update `deny.toml`'s allow list if a
new license appears.

Echo itself is licensed **`MIT OR Apache-2.0`** (see the root `Cargo.toml`).
All third-party crates below are permissive-licensed; the few `OR`/`AND`
expressions always include a permissive alternative (e.g. MIT or Apache-2.0).

> **libmpv note:** the platform player binary (libmpv, LGPL-2.1+ / ISC) is
> bundled separately at packaging time and is **not** a Cargo or npm
> dependency of this repository. Its source, checksum, ABI and license are
> pinned and checked individually by task 1.10; it is intentionally not listed
> here.

---

## Workspace members

| Crate | Version | License | Source |
| --- | --- | --- | --- |
| `echo-app` | 0.1.0 | MIT OR Apache-2.0 | workspace (`apps/desktop/src-tauri`) |
| `echo-core` | 0.1.0 | MIT OR Apache-2.0 | <https://github.com/echo-player/echo> |
| `echo-desktop` | 0.1.0 | MIT OR Apache-2.0 | <https://github.com/echo-player/echo> |

## Rust transitive dependencies (from `Cargo.lock`)

Full lockfile graph, including dev-dependencies and target-specific crates.
Policy enforcement happens separately via `cargo deny check` (see
`deny.toml`); the set shown here is the superset.

| Crate | Version | License | Source |
| --- | --- | --- | --- |
| bitflags | 2.13.1 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 | https://github.com/fitzgen/bumpalo |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| errno | 0.3.14 | MIT OR Apache-2.0 | https://github.com/lambda-fairy/rust-errno |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-task | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| itoa | 1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| js-sys | 0.3.104 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys |
| libc | 0.2.189 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/sunfishcode/linux-raw-sys |
| memchr | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| quote | 1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| rustix | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/rustix |
| rustversion | 1.0.23 | MIT OR Apache-2.0 | https://github.com/dtolnay/rustversion |
| serde | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| slab | 0.4.12 | MIT | https://github.com/tokio-rs/slab |
| syn | 2.0.119 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| syn | 3.0.4 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | https://github.com/Stebalien/tempfile |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thiserror-impl | 2.0.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| tracing | 0.1.44 | MIT | https://github.com/tokio-rs/tracing |
| tracing-attributes | 0.1.31 | MIT | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | https://github.com/tokio-rs/tracing |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| uuid | 1.26.0 | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid |
| wasm-bindgen | 0.2.127 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen |
| wasm-bindgen-macro | 0.2.127 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro |
| wasm-bindgen-macro-support | 0.2.127 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro-support |
| wasm-bindgen-shared | 0.2.127 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| zmij | 1.0.23 | MIT | https://github.com/dtolnay/zmij |

The license expressions in the tables are SPDX identifiers as declared by each
package's manifest. Cargo-deny cross-checks them against the license text
shipped in each crate when the expression alone is not authoritative, and
mirrors `cargo audit` for the RustSec advisory database.

## Frontend production dependencies (from `pnpm-lock.yaml`, `apps/desktop`)

Only **production** (`dependencies`) packages are shipped; build-time dev
dependencies (Vite, TypeScript, ESLint, Vitest, etc.) are not part of the
repackaged artifact license surface.

| Package | Version | License | Source |
| --- | --- | --- | --- |
| react | 18.3.1 | MIT | https://github.com/facebook/react |
| react-dom | 18.3.1 | MIT | https://github.com/facebook/react |

---

## Regeneration

To regenerate this file from the live lockfiles:

```sh
# Rust graph
cargo metadata --format-version 1   # -> packages[].{name,version,license,repository|homepage}

# Frontend prod set
cd apps/desktop && pnpm list --prod --depth 0 --json
```

The check script does **not** regenerate this file on every run to avoid diff
churn; it only verifies existence and non-emptiness. After a deliberate
dependency change, regenerate and commit an updated `LICENSES.md`.