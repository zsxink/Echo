#!/usr/bin/env node
// Task 2.8 check: stable sort descriptions, server-side cursor and playback
// context types.
//
// Fails unless the `catalog` tests pass and at least one actually ran: the four
// sorts have a strict tie-break (total order), the cursor is opaque and
// revision-guarded, and the playback-context request stays small even for a
// 50,000-song view (no Vec<SongId> over IPC). Shared fmt/clippy gates live in
// quality-core.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 2.8: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", "catalog::"], { cwd: ROOT, encoding: "utf8" });
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// test would leave this check green while proving nothing.
const passed = [...`${test.stdout}${test.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (passed === 0) fail("cargo test", `filter 'catalog::' matched no tests\n${test.stdout}\n${test.stderr}`);

process.stdout.write("ok 2.8: stable sort + opaque server-side cursor + playback context types\n");
