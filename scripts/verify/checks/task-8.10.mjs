#!/usr/bin/env node
// Task 8.10 check: 真实单调时钟累计听歌时长 + 幂等 RecordPlayback 的生产接线。
//
// Fails unless:
//   1. the accumulator's threshold/idempotence/temporary-exclusion tests pass
//      over the real module (player::recording);
//   2. the production watcher fires exactly once past the threshold on the
//      REAL monotonic clock (no force_accumulate) and never for temporary
//      items (runtime::player stats_recorder tests);
//   3. Core's idempotent sink (recorded_play_sessions + play_count) passes its
//      SQLite test;
//   4. main.rs actually spawns the stats watcher wired to CorePlaybackRecorder
//      — the missing half that used to leave play_count at zero and 最近播放
//      empty forever;
//   5. `cargo fmt` is clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.10: ${step}: ${msg}\n`);
  process.exit(1);
}

function run(command, args) {
  const r = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (r.status !== 0) {
    fail(`${command} ${args.join(" ")}`, `${r.stdout}\n${r.stderr}`);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1 + 2. Accumulator module tests and the real-clock watcher tests.
const lib = run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--lib", "recording"]);
for (const name of ["library_session_records_once_reaching_threshold"]) {
  if (!lib.includes(name)) fail("accumulator tests", `missing test ${name}`);
}
const watcher = run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--lib", "stats_recorder"]);
for (const name of [
  "stats_recorder_records_a_qualified_library_listen_exactly_once",
  "stats_recorder_never_records_temporary_items",
]) {
  if (!watcher.includes(name)) fail("watcher tests", `missing test ${name}`);
}

// 3. Core sink idempotence.
run("cargo", [
  "test",
  "-p",
  "echo-core",
  "--all-features",
  "--lib",
  "playback_sessions_are_idempotent_and_writer_allows_concurrent_reads",
]);

// 4. Production wiring.
const mainSrc = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
for (const needle of ["player::spawn_stats_recorder(", "CorePlaybackRecorder::new("]) {
  if (!mainSrc.includes(needle)) {
    fail("wiring", `main.rs is missing ${needle} — play counts would never move`);
  }
}

// 5. fmt
run("cargo", ["fmt", "--all", "--", "--check"]);

process.stdout.write("ok 8.10: monotonic listen-time accumulation wired to idempotent RecordPlayback\n");
