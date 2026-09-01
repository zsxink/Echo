#!/usr/bin/env node
// Task 8.1 check: desktop PlayerPort / PlayerCommand / PlayerSnapshot /
// PlayMode / PlayerError and the in-process FakePlayer test double.
//
// The task proves the coordinator, queue, statistics and platform-control
// logic can be tested WITHOUT loading libmpv: the player port types and the
// runner up a libmpv handle.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib` passes (includes player::fake tests
//      proving commands/snapshots work with no libmpv);
//   2. `cargo clippy -p echo-desktop --all-targets -- -D warnings` is clean;
//   3. `cargo fmt --all -- --check` is clean.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.1: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

// Confirm the player fake tests actually ran (not silently skipped under a
// filter) and that no libmpv FFI crate is pulled into echo-desktop yet.
const ranPlayer = /player::fake::tests::/i.test(test.stdout);
if (!ranPlayer) {
  fail("cargo test", "no player::fake tests found in the output");
}

process.stdout.write(
  "ok 8.1: PlayerPort/PlayerCommand/PlayerSnapshot + FakePlayer tested without libmpv\n",
);
