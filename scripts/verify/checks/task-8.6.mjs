#!/usr/bin/env node
// Task 8.6 check: queueEntryId-keyed current/history/shuffle bag, sequential/
// shuffle/repeat-one modes, ">5s previous restarts current".
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player::coordinator` passes the
//      mode/shuffle/previous tests;
//   2. `cargo test -p echo-desktop --lib player::queue` passes the
//      bag/mode-advance tests;
//   3. the queue/coordinator implement the required surfaces (shuffle bag
//      keyed by QueueEntryId, advance_in_mode, set_mode, previous with the
//      5s restart threshold);
//   4. `cargo clippy` and `cargo fmt` are clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.6: ${step}: ${msg}\n`);
  process.exit(1);
}

// 1. Coordinator tests
const coordTest = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player::coordinator"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (coordTest.status !== 0) fail("coordinator tests", `${coordTest.stdout}\n${coordTest.stderr}`);
for (const name of [
  "shuffle_round_plays_no_repeated_entry_while_bag_last",
  "duplicate_song_in_shuffle_round_stays_distinct",
  "repeat_one_repeats_current_on_end",
  "explicit_next_leaves_repeat_one",
  "mode_switch_keeps_current_item",
  "previous_restarts_current_after_5_seconds",
  "previous_moves_back_within_5_seconds",
]) {
  if (!coordTest.stdout.includes(name)) fail("coordinator tests", `missing test ${name}`);
}

// 2. Queue tests
const queueTest = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player::queue"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (queueTest.status !== 0) fail("queue tests", `${queueTest.stdout}\n${queueTest.stderr}`);
for (const name of [
  "shuffle_bag_pops_in_selected_order_once_each",
  "shuffle_bag_never_repeats_within_a_round",
  "sequential_mode_ignores_shuffle_bag",
  "repeat_one_returns_current_forever",
]) {
  if (!queueTest.stdout.includes(name)) fail("queue tests", `missing test ${name}`);
}

// 3. Required surfaces (queueEntryId-keyed shuffle bag + mode-aware advance +
//    5s previous threshold).
const qSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "queue.rs"), "utf8");
for (const needle of [
  "shuffle_bag: Vec<QueueEntryId>",
  "pub fn advance_in_mode",
  "pub fn set_shuffle",
  "pub fn shuffle_bag(",
]) {
  if (!qSrc.includes(needle)) fail("queue surface", `missing ${needle}`);
}

const cSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "coordinator.rs"), "utf8");
for (const needle of [
  "pub fn set_mode",
  "PREVIOUS_RESTART_THRESHOLD",
  "pub fn previous",
  "pub trait ShuffleSource",
]) {
  if (!cSrc.includes(needle)) fail("coordinator surface", `missing ${needle}`);
}

// 4. Clippy and fmt
const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.6: queueEntryId shuffle bag, 3 modes, >5s previous, no fold/re-shuffle/mode-loss\n",
);
