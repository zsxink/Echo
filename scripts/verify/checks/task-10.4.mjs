#!/usr/bin/env node
// Task 10.4 check: 壳层布局（侧边导航 / 顶部栏 / 资料库工作区 / 常驻播放栏）
// 且一期范围守卫成立（同步、全选批量、手动歌单排序、歌曲编辑入口不渲染）。
//
// Fails unless:
//   1. the shell suites (App / overlays / narrow / accessibility) pass;
//   2. App.test.tsx still carries the named no-sync-entry scope guard.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(step, msg) {
  process.stderr.write(`FAIL 10.4: ${step}: ${msg}\n`);
  process.exit(1);
}

const appTestPath = resolve(APP, "src", "app", "App.test.tsx");
const appTest = readFileSync(appTestPath, "utf8");
if (!appTest.includes("renders no sync entry anywhere in the activated shell")) {
  fail("scope guard", "App.test.tsx lost the named 同步 scope-guard assertion");
}

for (const filter of [
  "src/app/App.test.tsx",
  "src/app/overlays.test.tsx",
  "src/app/narrow.test.tsx",
  "src/app/accessibility.test.tsx",
]) {
  const r = spawnSync("pnpm", ["--filter", "@echo/desktop", "test", filter], {
    cwd: APP,
    encoding: "utf8",
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) fail("vitest", `${filter}\n${out}`);
  const files = Number((out.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) fail("vitest", `'${filter}' matched ${files} test files (expected exactly 1)`);
}

process.stdout.write("ok 10.4: shell layout renders and phase-one scope guard holds\n");
