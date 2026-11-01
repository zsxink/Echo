#!/usr/bin/env node
// Task 10.3 check: 资料库工作区状态机（首次启动 / 未配置 / 候选扫描 / 只读 /
// 不可用）与选择取消、切换失败保留旧库、写操作禁用。
//
// Fails unless:
//   1. the useLibraryStatus stale-response suite passes;
//   2. the shell suite naming the workspace states passes (first-launch
//      choose-root, picker cancellation, activation-failure retry);
//   3. the read-only branch is real: LibraryWorkspace renders disabled writes
//      when readOnly (write actions must be unusable on a read-only gate).

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(step, msg) {
  process.stderr.write(`FAIL 10.3: ${step}: ${msg}\n`);
  process.exit(1);
}

function runDesktopTest(filter) {
  const r = spawnSync("pnpm", ["--filter", "@echo/desktop", "test", filter], {
    cwd: APP,
    encoding: "utf8",
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) fail("vitest", `${filter}\n${out}`);
  // Vitest colours its summary whenever `CI` is set, so `Test Files <esc>[22m 1`
  // defeats the count regex unless the styling is stripped first.
  const clean = out.replace(/\u001B\[[0-9;]*[A-Za-z]/g, "");
  const files = Number((clean.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) fail("vitest", `'${filter}' matched ${files} test files (expected exactly 1)`);
}

runDesktopTest("src/features/workspace/useLibraryStatus.test.ts");
runDesktopTest("src/app/App.test.tsx");

// 3. The workspace states are named where they live.
const appTest = readFileSync(resolve(APP, "src", "app", "App.test.tsx"), "utf8");
for (const needle of [
  "first-launch choose-root view when no library is configured",
  "keeps selection available after cancellation",
  "allows retry if the post-activation status query fails",
]) {
  if (!appTest.includes(needle)) fail("shell states", `App.test.tsx lost '${needle}'`);
}
// 3. The read-only branch is real: the workspace threads `readOnly` into every
//    surface that can write, and those surfaces gate their write actions on it.
//    The old single-regex pinned one rendering shape (`!readOnly ? ( ... )`);
//    the component now gates per-surface, so assert each surface instead —
//    dropping any one of them makes a write usable on a read-only library.
const workspace = readFileSync(
  resolve(APP, "src", "features", "library", "LibraryWorkspace.tsx"),
  "utf8",
);
if (!/<SongList[\s\S]*?readOnly={readOnly}/.test(workspace)) {
  fail("read-only", "LibraryWorkspace does not pass readOnly to SongList");
}
if (!/<SongMenu[\s\S]*?readOnly={readOnly}/.test(workspace)) {
  fail("read-only", "LibraryWorkspace does not pass readOnly to SongMenu");
}

const songList = readFileSync(
  resolve(APP, "src", "features", "library", "SongList.tsx"),
  "utf8",
);
// The empty-library import entry point must disappear entirely when read-only.
if (!/!\s*readOnly\s*&&\s*props\.onImport/.test(songList)) {
  fail("read-only", "SongList renders the import entry point regardless of readOnly");
}
if (!/readOnly={readOnly}/.test(songList)) {
  fail("read-only", "SongList does not thread readOnly down to its rows");
}

const songRow = readFileSync(
  resolve(APP, "src", "features", "library", "SongRow.tsx"),
  "utf8",
);
if (!/disabled={readOnly \|\| unavailable}/.test(songRow)) {
  fail("read-only", "SongRow's favorite control is not disabled on a read-only library");
}

const songMenu = readFileSync(
  resolve(APP, "src", "features", "library", "SongMenu.tsx"),
  "utf8",
);
if (!/disabled: readOnly/.test(songMenu)) {
  fail("read-only", "SongMenu does not disable its write items on a read-only library");
}
// Destructive actions are not merely disabled: they are absent.
if (!/\.\.\.\(readOnly[\s\S]*?song-menu-delete/.test(songMenu)) {
  fail("read-only", "SongMenu still offers delete on a read-only library");
}

process.stdout.write("ok 10.3: workspace states (unconfigured / read-only / error retry) hold\n");
