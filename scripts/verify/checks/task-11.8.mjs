#!/usr/bin/env node
// Task 11.8 check: app shortcuts, range keyboard stepping and media status text.
//
// Acceptance (desktop-playback §媒体键与快捷键 / desktop-app-shell §键盘焦点):
//  - Global app shortcuts (Space = play/pause, ,/. = prev/next, M = mute,
//    arrows = seek/volume) map to the on-screen controls.
//  - Space in an input/dialog must NOT trigger playback; shortcuts stand down
//    while an overlay (queue/immersive/focus) owns the keys.
//  - Seek range steps 5 seconds; volume range steps 5%.
//  - A live region announces media status (playing/paused + song + mode).
// This check gates on the hotkey tests + PlayerBar range/status tests +
// typecheck + echo-app clippy.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.8: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  [
    "--filter",
    "@echo/desktop",
    "test",
    "--",
    "--run",
    "src/player/useGlobalPlayerHotkeys.test.tsx src/features/player/PlayerBar.test.tsx",
  ],
  APP,
);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 11.8: app shortcuts + range stepping + media status verified\n");
