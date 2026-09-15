#!/usr/bin/env node
// Task 11.6 check: lyrics focus mode.
//
// Acceptance (immersive-lyrics spec §歌词专注模式 / §歌词滚动):
//  - Enter focus from the immersive lyrics area; hide non-essential cover/meta
//    and let the lyrics take the surface. Focus is the prototype's
//    `body.lyrics-focus-open` *state of the same surface*, not a second panel;
//    the 常驻播放栏 below keeps the minimal playback controls.
//  - Exit focus via Escape / the 收起 button / re-activating the lyrics area;
//    Escape unwinds focus first, not the immersive player.
//  - Manual scroll pauses auto-scroll 5 s; "回到当前行" (or time advance) resumes.
//
// This check gates on the ImmersivePlayer tests (which cover enter/hide/exit/
// restore) plus typecheck + echo-app clippy. The *rendered* focus layout is
// asserted in the browser by e2e/run-e2e.mjs (PRD A9): a unit test can prove
// the state flipped while the surface stays invisible, which is exactly how
// this shipped broken once.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.6: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/ImmersivePlayer.test.tsx"],
  APP,
);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 11.6: lyrics focus mode (enter/take-surface/exit/restore) verified\n");
