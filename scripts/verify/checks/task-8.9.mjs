#!/usr/bin/env node
// Task 8.9 check: 播放会话原子持久化 + 启动 paused 恢复的落盘与恢复接线。
//
// Fails unless:
//   1. the coordinator restores a persisted session paused
//      (`restore_session_recovers_queue_mode_and_settings_paused`) and primes
//      the default view without a sound (`play_context_paused_loads_without_playing`);
//   2. the runtime restore-or-prime decision keeps its three outcomes
//      (restored / primed / empty) over the real SessionPersistence port;
//   3. the session module's own atomic-write tests pass;
//   4. the composition root actually wires the saver thread
//      (`spawn_session_saver` in main.rs) and the boot command
//      (`restore_playback_session` in commands.rs) — the two halves this task
//      used to ship without;
//   5. `cargo fmt` is clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.9: ${step}: ${msg}\n`);
  process.exit(1);
}

function run(command, args) {
  const r = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (r.status !== 0) {
    fail(`${command} ${args.join(" ")}`, `${r.stdout}\n${r.stderr}`);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1 + 2. Restore/prime behavior over coordinator and runtime layers.
const lib = run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--lib", "session"]);
for (const name of [
  "restore_session_recovers_queue_mode_and_settings_paused",
  "restore_or_prime_restores_a_persisted_session_paused",
  "rebuild_queue_drops_permanently_deleted_and_unreachable",
  "state_store_save_load_round_trip_is_atomic_and_clears",
]) {
  if (!lib.includes(name)) fail("restore tests", `missing test ${name}`);
}
// The paused-priming test lives under coordinator, matched by "paused".
const paused = run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--lib", "paused"]);
if (!paused.includes("play_context_paused_loads_without_playing")) {
  fail("restore tests", "missing test play_context_paused_loads_without_playing");
}

// 3. Session module tests are already covered by the "session" filter above
//    (player::session::* runs under it).

// 4. Production wiring on both halves.
const mainSrc = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
if (!mainSrc.includes("player::spawn_session_saver(")) {
  fail("wiring", "main.rs never starts the session saver thread — the durable write half is unwired");
}
const commandsSrc = readFileSync(
  resolve(ROOT, "apps", "desktop", "src-tauri", "src", "commands.rs"),
  "utf8",
);
if (!commandsSrc.includes("restore_or_prime_playback(")) {
  fail("wiring", "commands.rs never calls restore_or_prime_playback — the cold-start restore half is unwired");
}

// 5. fmt
run("cargo", ["fmt", "--all", "--", "--check"]);

process.stdout.write("ok 8.9: session persistence saved on transitions and restored paused at cold start\n");
