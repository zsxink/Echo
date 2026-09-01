#!/usr/bin/env node
// Task 8.5 check: PlaybackCoordinator view context, current/history, join
// queue, next-track, clear pending, error skip, duplicate SongId independent
// queueEntryId.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player::coordinator` passes all 6
//      coordinator tests;
//   2. `cargo test -p echo-desktop --lib player::queue` passes all 9 queue
//      tests;
//   3. `PlaybackCoordinator` has the required API surface (play_context,
//      enqueue, play_next, advance_to_next, clear_pending, on_played_to_end,
//      play_temporary);
//   4. `cargo clippy` and `cargo fmt` are clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.5: ${step}: ${msg}\n`);
  process.exit(1);
}

// 1. Coordinator tests
const coordTest = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player::coordinator"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (coordTest.status !== 0) fail("coordinator tests", `${coordTest.stdout}\n${coordTest.stderr}`);
for (const name of [
  "play_context_loads_selected_song",
  "duplicate_song_can_appear_multiple_times_in_queue",
  "clear_pending_keeps_current_and_queue_of_one",
  "next_advances_in_order",
  "error_end_auto_skips_to_next_available",
  "play_temporary_is_session_only",
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
  "duplicate_song_ids_get_independent_entry_ids",
  "insert_next_goes_immediately_after_current",
  "advance_next_plays_in_order_and_exhausts",
  "clear_pending_keeps_current_and_history",
  "remove_pending_only",
  "previous_returns_history_entry",
  "view_context_builds_queue_with_selected_current",
  "selected_index_clamped",
  "empty_view_yields_empty_queue",
]) {
  if (!queueTest.stdout.includes(name)) fail("queue tests", `missing test ${name}`);
}

// 3. PlaybackCoordinator API surface
const coordSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "coordinator.rs"), "utf8");
for (const method of [
  "pub fn play_context",
  "pub fn enqueue",
  "pub fn play_next",
  "pub fn advance_to_next",
  "pub fn clear_pending",
  "pub fn on_played_to_end",
  "pub fn play_temporary",
  "pub fn previous",
]) {
  if (!coordSrc.includes(method)) fail("API surface", `missing ${method}`);
}

// 4. ViewContext::build_queue must exist in queue.rs
const queueSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "queue.rs"), "utf8");
if (!queueSrc.includes("pub fn build_queue")) fail("API surface", "missing ViewContext::build_queue");

// 5. Clippy and fmt
const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.5: PlaybackCoordinator view context, queue, history, error skip, duplicate SongId\n",
);
