#!/usr/bin/env node
// Task 2.8 check: stable sort descriptions, server-side cursor and playback
// context types.
//
// Fails unless:
//   1. `cargo test -p echo-core` passes — the catalog tests prove the four
//      sorts have a strict tie-break (total order), the cursor is opaque and
//      revision-guarded, and the playback-context request stays small even for
//      a 50,000-song view (no Vec<SongId> over IPC);
//   2. clippy is clean;
//   3. fmt is clean.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.8: ${step}: ${msg}\n`);
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

process.stdout.write("ok 2.8: stable sort + opaque server-side cursor + playback context types\n");
