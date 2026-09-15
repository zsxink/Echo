#!/usr/bin/env node
// Task 8.11 check: 桌面删除协调器（快照 / unload 屏障 / 提交 / 回滚）的生产接线。
//
// Fails unless:
//   1. the DeletionCoordinator's snapshot/commit/rollback semantics pass its
//      module tests (player::deletion);
//   2. the production coordinated delete commits with the queue aligned and
//      rolls back (keeping the current entry) when Core refuses
//      (runtime::player coordinated_delete tests);
//   3. the command layer routes deletes through delete_song_coordinated —
//      calling services.delete_song directly used to bypass the player
//      entirely (no unload barrier, no queue alignment);
//   4. the actor's Stop actually silences the backend (a Pause(true) write),
//      not only its own state flag;
//   5. `cargo fmt` is clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.11: ${step}: ${msg}\n`);
  process.exit(1);
}

function run(command, args) {
  const r = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (r.status !== 0) {
    fail(`${command} ${args.join(" ")}`, `${r.stdout}\n${r.stderr}`);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1 + 2. Coordinator semantics and the production commit/rollback proofs.
const lib = run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--lib", "deletion"]);
const coordinated = run("cargo", [
  "test",
  "-p",
  "echo-desktop",
  "--all-features",
  "--lib",
  "coordinated_delete",
]);
for (const name of [
  "coordinated_delete_removes_current_song_and_commits",
  "coordinated_delete_rolls_back_when_core_refuses",
]) {
  if (!coordinated.includes(name)) fail("coordinated delete tests", `missing test ${name}`);
}

// 3. Command-layer routing.
const commandsSrc = readFileSync(
  resolve(ROOT, "apps", "desktop", "src-tauri", "src", "commands.rs"),
  "utf8",
);
if (!commandsSrc.includes("delete_song_coordinated(")) {
  fail("wiring", "commands.rs delete_song bypasses the DeletionCoordinator");
}

// 4. Actor Stop silences the backend.
const actorSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor.rs"), "utf8");
const stopArm = actorSrc.match(/PlayerCommand::Stop => \{[\s\S]*?\n            \}/);
if (!stopArm || !stopArm[0].includes("BackendProperty::Pause(true)")) {
  fail("actor stop", "PlayerCommand::Stop does not write Pause(true) — mpv would keep playing");
}

// 5. fmt
run("cargo", ["fmt", "--all", "--", "--check"]);

process.stdout.write("ok 8.11: deletion coordinated with the player (snapshot / unload / commit / rollback)\n");
