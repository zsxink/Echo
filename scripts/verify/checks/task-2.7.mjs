#!/usr/bin/env node
// Task 2.7 check: test doubles for all ports.
//
// Fails unless:
//   1. The `testkit` feature builds and its fakes compile: `cargo clippy -p
//      echo-core --all-targets -- -D warnings` (fakes live behind `cfg(test)`
//      and the `testkit` feature);
//   2. `cargo test -p echo-core` passes, including application::testing::tests
//      which exercise permission revocation (fault injection), trash failure,
//      out-of-order/duplicate watcher events and temp-dir file system — none
//      touching a real user directory;
//   3. fmt is clean.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.7: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core"], { cwd: ROOT, encoding: "utf8" });
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write("ok 2.7: port test doubles (fault/trash/watcher/temp-dir)\n");
