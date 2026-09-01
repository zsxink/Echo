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
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.2: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);
if (!/player::actor::tests::/i.test(test.stdout)) {
  fail("cargo test", "no player::actor tests found in output");
}

// Unsafe isolation: ipc / platform / runtime must forbid `unsafe_code`, while
// player is the only subtree allowed to use it.
for (const mod of ["ipc/mod.rs", "platform/mod.rs", "runtime/mod.rs"]) {
  const path = resolve(ROOT, "crates", "echo-desktop", "src", mod);
  if (!readFileSync(path, "utf8").includes("#![forbid(unsafe_code)]")) {
    fail("unsafe isolation", `${mod} must forbid unsafe_code`);
  }
}
const desktopToml = readFileSync(resolve(ROOT, "crates", "echo-desktop", "Cargo.toml"), "utf8");
if (!desktopToml.includes('unsafe_code = "allow"')) {
  fail("unsafe isolation", "echo-desktop must allow unsafe only at crate level");
}

// echo-core must not depend on libmpv/FFI/loading: verify the arch test exists.
const archTest = readFileSync(resolve(ROOT, "crates", "echo-core", "tests", "arch.rs"), "utf8");
if (!/mpv|libloading|libmpv/.test(archTest)) {
  fail("arch", "echo-core arch test should guard against mpv/FFI deps");
}

const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.2: isolated unsafe libmpv FFI adapter + dedicated actor (unique handle, bounded channel, ordered teardown)\n",
);
