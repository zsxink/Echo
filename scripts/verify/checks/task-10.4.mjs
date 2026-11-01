#!/usr/bin/env node
// Task 10.4 check: 壳层布局（侧边导航 / 顶部栏 / 资料库工作区 / 常驻播放栏）
// 且一期范围守卫成立（同步未启用时不可操作、全选批量、手动歌单排序、歌曲编辑
// 入口不渲染）。
//
// Fails unless:
//   1. the shell suites (App / overlays / narrow / accessibility) pass;
//   2. App.test.tsx still carries the named sync opt-in guard, and that guard
//      still asserts the inoperable state (「已关闭」且没有立即/暂停同步),
//      not merely the absence of a UI entry.
//
// The old guard asserted 「壳层不渲染同步入口」. Sync now lives behind
// 设置 → 同步 (spec: 尚未启用时不建立网络连接), so the guard follows the
// behaviour that is actually specified instead of pinning a removed surface.

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

const GUARD_NAME = "keeps sync inoperable until the user enables it in settings";
const appTestPath = resolve(APP, "src", "app", "App.test.tsx");
const appTest = readFileSync(appTestPath, "utf8");
if (!appTest.includes(GUARD_NAME)) {
  fail("scope guard", `App.test.tsx lost the named sync opt-in guard '${GUARD_NAME}'`);
}
const guardStart = appTest.indexOf(GUARD_NAME);
const guardBody = appTest.slice(guardStart, appTest.indexOf("\n  });", guardStart));
if (!guardBody.includes("已关闭") || !guardBody.includes("lan-sync-now")) {
  fail(
    "scope guard",
    "the sync opt-in guard no longer asserts the inoperable state (已关闭 / no 立即同步)",
  );
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
  // Vitest colours its summary whenever `CI` is set, so `Test Files <esc>[22m 1`
  // defeats the count regex unless the styling is stripped first.
  const clean = out.replace(/\u001B\[[0-9;]*[A-Za-z]/g, "");
  const files = Number((clean.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) fail("vitest", `'${filter}' matched ${files} test files (expected exactly 1)`);
}

process.stdout.write("ok 10.4: shell layout renders and phase-one scope guard holds\n");
