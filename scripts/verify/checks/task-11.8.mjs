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
//
// `--static-only` (release gate) keeps the key-map / test-name drift guard and
// skips typecheck + Vitest: the Frontend job runs both suites in full.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");
const staticOnly = process.argv.includes("--static-only");

function fail(msg) {
  process.stderr.write(`FAIL 11.8: ${msg}\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.8: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

// --- Static drift guard: the key map, the input/overlay guards and the range
//     steps still exist; the behavior is exercised by the frontend suites.
const hotkeys = readFileSync(resolve(APP, "src", "player", "useGlobalPlayerHotkeys.ts"), "utf8");
for (const token of [
  'event.code === "Space"',
  "isEditableTarget",
  "hasOverlayOpen",
  'case "ArrowLeft"',
  'case "ArrowRight"',
  'case "ArrowUp"',
  'case "ArrowDown"',
  'case "m"',
  'case "M"',
  "seekRelative(-5)",
  "seekRelative(5)",
  "volumeRelative(0.05)",
  "volumeRelative(-0.05)",
  'bridge.fireAndForget("toggle_mute")',
]) {
  if (!hotkeys.includes(token)) fail(`useGlobalPlayerHotkeys.ts lost '${token}'`);
}
const hotkeyTest = readFileSync(
  resolve(APP, "src", "player", "useGlobalPlayerHotkeys.test.tsx"),
  "utf8",
);
for (const name of [
  "Space toggles play/pause",
  "Space does NOT toggle when focus is inside an input",
  "Up arrow steps volume +5% and clamps at 1",
  "hotkeys are ignored while the queue overlay is open",
  ". advances to next and , goes to previous",
]) {
  if (!hotkeyTest.includes(name)) fail(`useGlobalPlayerHotkeys.test.tsx lost '${name}'`);
}
const playerBarTest = readFileSync(
  resolve(APP, "src", "features", "player", "PlayerBar.test.tsx"),
  "utf8",
);
for (const name of ["volume range steps by 5%", "announces media status via a live region"]) {
  if (!playerBarTest.includes(name)) fail(`PlayerBar.test.tsx lost '${name}'`);
}

if (!staticOnly) {
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
  process.stdout.write("ok 11.8: app shortcuts + range stepping + media status verified\n");
} else {
  process.stdout.write(
    "ok 11.8: shortcut map + range-step wiring intact (static-only; behavior delegated to frontend-test)\n",
  );
}
