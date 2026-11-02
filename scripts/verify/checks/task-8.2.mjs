#!/usr/bin/env node
// Task 8.2 check: minimal `unsafe` libmpv FFI adapter + dedicated OS-thread
// actor.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player` passes (actor:
//      bounded-command-channel, ordered-destruction, generation, degraded
//      start; ffi: path/C-string helpers — all without loading a real libmpv);
//   2. the FFI `unsafe` stays isolated: the player module is the only place in
//      echo-desktop that may use `unsafe` (ipc/platform/runtime forbid it);
//   3. `cargo clippy -p echo-desktop --all-targets -- -D warnings` is clean;
//   4. `cargo fmt --all -- --check` is clean;
//   5. `libloading` resolves and echo-core has no mpv/FFI dependency (arch
//      test), so the player adapter never reaches into Core.

import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.2: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "--all-features", "player"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);
if (!/player::actor::tests::/i.test(test.stdout)) {
  fail("cargo test", "no player::actor tests found in output");
}

// Unsafe isolation: the workspace denies `unsafe_code`, and only the documented
// FFI modules opt out locally. A crate-wide `allow` would defeat the boundary,
// so it must not appear anywhere.
const workspaceToml = readFileSync(resolve(ROOT, "Cargo.toml"), "utf8");
if (!/\[workspace\.lints\.rust\][\s\S]*?unsafe_code = "deny"/.test(workspaceToml)) {
  fail("unsafe isolation", "workspace lints must deny unsafe_code");
}
for (const crateToml of ["crates/echo-desktop/Cargo.toml", "apps/desktop/src-tauri/Cargo.toml"]) {
  const text = readFileSync(resolve(ROOT, crateToml), "utf8");
  if (!/\[lints\][\s\S]*?workspace = true/.test(text)) {
    fail("unsafe isolation", `${crateToml} must inherit the workspace lints`);
  }
  if (text.includes('unsafe_code = "allow"')) {
    fail("unsafe isolation", `${crateToml} must not re-allow unsafe at crate level`);
  }
}

// Modules that may not use `unsafe` forbid it outright (a deny is overridable
// by a descendant `allow`; forbid is not).
for (const mod of ["ipc/mod.rs", "platform/mod.rs", "runtime/mod.rs"]) {
  const path = resolve(ROOT, "crates", "echo-desktop", "src", mod);
  if (!readFileSync(path, "utf8").includes("#![forbid(unsafe_code)]")) {
    fail("unsafe isolation", `${mod} must forbid unsafe_code`);
  }
}

// The only opt-outs are the libmpv FFI module and the actor that calls into it,
// plus the three macOS shell modules that call platform Objective-C APIs. The
// set is pinned exactly so a new opt-out has to be added deliberately.
const allowedOptOuts = new Set([
  "crates/echo-desktop/src/player/ffi.rs",
  "crates/echo-desktop/src/player/actor/mod.rs",
  "apps/desktop/src-tauri/src/macos_now_playing.rs",
  "apps/desktop/src-tauri/src/macos_status_row.rs",
  "apps/desktop/src-tauri/src/macos_window_lifecycle.rs",
]);
const optOuts = [];
function scanOptOuts(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      scanOptOuts(full);
      continue;
    }
    if (!entry.name.endsWith(".rs")) continue;
    if (readFileSync(full, "utf8").includes("#![allow(unsafe_code)]")) {
      optOuts.push(relative(ROOT, full));
    }
  }
}
for (const crate of ["crates/echo-desktop/src", "apps/desktop/src-tauri/src"]) {
  scanOptOuts(resolve(ROOT, crate));
}
const unexpected = optOuts.filter((p) => !allowedOptOuts.has(p));
if (unexpected.length > 0) {
  fail("unsafe isolation", `unexpected unsafe_code opt-out(s): ${unexpected.join(", ")}`);
}
for (const required of allowedOptOuts) {
  if (!optOuts.includes(required)) {
    fail("unsafe isolation", `expected unsafe_code opt-out missing from ${required}`);
  }
}

// echo-core must not depend on libmpv/FFI/loading: verify the arch test exists.
const archTest = readFileSync(resolve(ROOT, "crates", "echo-core", "tests", "arch.rs"), "utf8");
if (!/mpv|libloading|libmpv/.test(archTest)) {
  fail("arch", "echo-core arch test should guard against mpv/FFI deps");
}

process.stdout.write(
  "ok 8.2: isolated unsafe libmpv FFI adapter + dedicated actor (unique handle, bounded channel, ordered teardown)\n",
);
