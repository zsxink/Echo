#!/usr/bin/env node
// Task 2.6 check: Ports boundary (no SQLite/Tauri/mpv types leaked).
//
// Fails unless the `arch` integration tests pass and at least one actually ran:
// they reject forbidden dependencies, upward layer edges and platform branches,
// which is what keeps SQLite/Tauri/mpv types below the ports. Shared fmt/clippy
// gates live in the governance gate's quality-core segment.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.6: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", "--test", "arch"], { cwd: ROOT, encoding: "utf8" });
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// test would leave this check green while proving nothing.
const passed = [...`${test.stdout}${test.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (passed === 0) fail("cargo test", `filter 'arch' matched no tests\n${test.stdout}\n${test.stderr}`);

process.stdout.write("ok 2.6: Ports boundary (no SQLite/Tauri/mpv types leaked)\n");
