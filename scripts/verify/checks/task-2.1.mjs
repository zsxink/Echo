#!/usr/bin/env node
// Task 2.1 check: domain ID newtypes (SongId, PlaylistId, LibraryRootId,
// OperationId, PlaybackSessionId, RelativeMediaPath, QueueEntryId, Revision).
//
// Fails unless:
//   1. `cargo test -p echo-core` passes (includes ids unit tests: serialization,
//      parse-failure and type-safety "cannot mix" tests);
//   2. `cargo clippy -p echo-core --all-targets -- -D warnings` is clean;
//   3. `cargo fmt --all -- --check` is clean.

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

const test = spawnSync("cargo", ["test", "-p", "echo-core"], {
  cwd: ROOT,
  encoding: "utf8",
  maxBuffer: MAX_BUFFER,
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);

const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8", maxBuffer: MAX_BUFFER },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], {
  cwd: ROOT,
  encoding: "utf8",
  maxBuffer: MAX_BUFFER,
});
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write("ok 2.1: domain ID newtypes with serialization/parse/type-safety tests\n");
