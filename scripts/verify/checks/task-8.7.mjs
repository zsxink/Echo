#!/usr/bin/env node
// Task 8.7 check: per-round error set keyed by queueEntryId; error advance
// bypasses repeat-one; shuffle only picks un-failed entries; each entry is
// auto-attempted at most once; all-corrupt stops without spinning.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player::coordinator` passes the
//      error-skip tests;
//   2. the coordinator exposes the required error-skip surface (on_load_error,
//      failed_round keyed by QueueEntryId);
//   3. `cargo clippy` and `cargo fmt` are clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.7: ${step}: ${msg}\n`);
  process.exit(1);
}

// 1. Coordinator tests
const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player::coordinator"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("coordinator tests", `${test.stdout}\n${test.stderr}`);
for (const name of [
  "error_skips_bad_entry_and_plays_next_sequential",
  "error_bypasses_repeat_one",
  "error_advance_never_retries_same_entry_in_round",
  "error_advance_all_bad_stops_without_spin",
  "error_advance_in_shuffle_skips_failed_entry",
]) {
  if (!test.stdout.includes(name)) fail("coordinator tests", `missing test ${name}`);
}

// 2. Error-skip surface keyed by queueEntryId
const src = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "coordinator.rs"), "utf8");
for (const needle of [
  "pub fn on_load_error",
  "failed_round: std::collections::HashSet<QueueEntryId>",
  "pub fn failed_round(&self)",
]) {
  if (!src.includes(needle)) fail("error-skip surface", `missing ${needle}`);
}

// 3. Clippy and fmt
const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.7: per-round error set, error advance bypasses repeat-one, no spin on all-corrupt\n",
);
