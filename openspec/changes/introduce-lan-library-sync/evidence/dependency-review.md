# echo-sync dependency review

Review date: 2026-10-13. Workspace MSRV: Rust 1.81.

| Crate | Locked version | Declared MSRV | License | Role |
|---|---:|---:|---|---|
| tokio | 1.53.2 | 1.71 | MIT | Async runtime/network IO |
| axum | 0.8.9 | 1.80 | MIT | HTTP server |
| reqwest | 0.12.28 | 1.64 | MIT OR Apache-2.0 | HTTP client with rustls manual roots |
| rustls | 0.23.45 | 1.71 | Apache-2.0 OR ISC OR MIT | TLS configuration/client |
| tokio-rustls | 0.26.6 | 1.71 | MIT OR Apache-2.0 | Tokio TLS listener |
| mdns-sd | 0.21.5 | 1.71 | Apache-2.0 OR MIT | DNS-SD discovery |

The direct candidates fit Rust 1.81 and the existing `cargo deny` license
policy. The HTTP client disables default features and enables rustls only;
discovery is isolated behind its own feature. `echo-sync` declares empty
default features, with client/server/discovery opt-in.

## Build evidence (2026-10-13 historical run)

- `cargo check --workspace --all-targets --all-features`: passed on the host
  macOS arm64 toolchain.
- `cargo check -p echo-sync --all-targets --all-features --target
  x86_64-pc-windows-gnu`: passed.
- Linux cross-target check was attempted, but the environment has no
  `x86_64-linux-gnu-gcc`; the `ring` build script stopped before crate
  compilation. Linux compatibility is therefore not yet established.
- `cargo deny check`: passed (advisories, bans, licenses, sources).

## MSRV qualification (2026-10-13 historical lockfile)

Rust 1.81.0 was installed and tested by using its `rustc` with the current
Cargo and locked workspace graph. The check fails before compiling because the
resolved graph includes packages requiring Rust 1.82–1.89. Examples include
`lofty 0.22.4` (1.85), `uuid 1.26.0` (1.85), `image 0.25.10` (1.88),
`hyper-rustls 0.27.10` (1.85), and `icu_* 2.3.x` (1.88). Many belong to
pre-existing workspace dependency ranges; some are transitive additions of
the candidate HTTP stack. Dependency constraints/lockfile must be brought
within 1.81 and rechecked before task 1.3 can close. Current-toolchain builds
are not evidence of MSRV compatibility.

Running Cargo 1.81 directly attempted to refresh the crates.io index and failed
with an SSL connection error. The current Cargo plus Rust 1.81 check above
provides the explicit incompatibility report without requiring that refresh.

## Revalidation — 2026-10-14

The current lockfile supersedes the earlier incompatibility result.
`RUSTC="$(rustup which --toolchain 1.81.0 rustc)" cargo check -p echo-sync
--all-features --locked --offline` passed with Rust 1.81.0 and the host Cargo.
This validates compilation of the current Core/network stack with Rust 1.81;
it does not establish Cargo 1.81 operation, the complete desktop MSRV, or
native Windows/Linux compatibility. Task 1.3 remains open.

`cargo deny check` now fails its advisories gate: quick-xml 0.38.4 triggers
RUSTSEC-2026-0194 and RUSTSEC-2026-0195; time 0.3.44 triggers
RUSTSEC-2026-0009. Bans, licenses and sources pass. The earlier all-green
result must not be used as current delivery evidence. No advisory exception
or lockfile change was introduced by this review.
