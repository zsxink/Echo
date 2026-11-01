#!/usr/bin/env node
// Task 2.1 check: domain ID newtypes (SongId, PlaylistId, LibraryRootId,
// OperationId, PlaybackSessionId, RelativeMediaPath, QueueEntryId, Revision).
//
// Fails unless the `domain::ids` tests pass (serialization, parse-failure and
// type-safety "cannot mix" tests) and at least one actually ran. Shared
// fmt/clippy gates live in the governance gate's quality-core segment.

import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const MAX_BUFFER = 64 * 1024 * 1024;

function fail(step, msg) {
  // Write synchronously: process.exit() drops large pending stderr writes when the
  // pipe is full, which hid the libtest panic detail of a CI failure.
  writeFileSync(2, `FAIL 2.1: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-core", "--all-features", "domain::ids::"], {
  cwd: ROOT,
  encoding: "utf8",
  maxBuffer: MAX_BUFFER,
});
if (test.status !== 0) {
  fail("cargo test", `filter 'domain::ids::' exited ${test.status}\n${test.stdout}\n${test.stderr}`);
}

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// test would leave this check green while proving nothing.
const passed = [...`${test.stdout}${test.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (passed === 0) fail("cargo test", `filter 'domain::ids::' matched no tests\n${test.stdout}\n${test.stderr}`);

process.stdout.write("ok 2.1: domain ID newtypes with serialization/parse/type-safety tests\n");
