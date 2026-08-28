#!/usr/bin/env node
// Task 2.4 check: state machines (scan/root/operation/playback) with property tests.
//
// Fails unless `cargo test -p echo-core` passes, clippy is clean and fmt is
// clean (the whole suite covers this phase's domain-layer work).

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.4: ${step}: ${msg}\n`);
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

process.stdout.write("ok 2.4: state machines (scan/root/operation/playback) with property tests\n");
