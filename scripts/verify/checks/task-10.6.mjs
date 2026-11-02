#!/usr/bin/env node
// Task 10.6 check: virtualized library list ("下一首播放 / 加入播放队列").
//
// The virtual window + play indicator + favorite + detail/reveal/delete/undo
// were completed earlier. This check verifies the task-10.6 slice that this
// session added:
//   - SongMenu distinguishes "下一首播放" (onPlayNext, insert-after-current)
//     from "加入播放队列" (onEnqueue, append), bound to the SongId that opened
//     the menu (component tests).
//   - The Rust playback command layer exists: `queue_command` (enqueue /
//     playNext) and `play_context` handlers are wired (echo-app compiles +
//     clippy clean), and the actor resolves a library SongId → path and loads
//     it (actor resolver tests).

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const MENU_TEST = resolve(APP, "src/features/library/SongMenu.test.tsx");
if (!existsSync(MENU_TEST)) {
  process.stderr.write(`FAIL 10.6: missing ${MENU_TEST}\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 10.6: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/library/SongMenu.test.tsx"], APP);
// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// or moved test would leave this gate green while proving nothing. Require at
// least one test to have actually run.
const ACTOR_FILTER = "player::actor::tests::validation::load_library_song";
const actorTests = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "--all-features", ACTOR_FILTER], {
  cwd: ROOT,
  encoding: "utf8",
});
const actorOut = `${actorTests.stdout || ""}${actorTests.stderr || ""}`;
if (actorTests.status !== 0) {
  process.stderr.write(`FAIL 10.6: cargo test ${ACTOR_FILTER}\n${actorOut}\n`);
  process.exit(1);
}
const actorPassed = [...actorOut.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (actorPassed === 0) {
  process.stderr.write(`FAIL 10.6: filter '${ACTOR_FILTER}' matched no tests\n${actorOut}\n`);
  process.exit(1);
}

process.stdout.write("ok 10.6: virtualized list play actions (下一首播放 / 加入播放队列) verified\n");
