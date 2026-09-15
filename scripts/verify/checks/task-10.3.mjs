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
  const files = Number((out.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
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
const workspace = readFileSync(resolve(APP, "src", "features", "library", "LibraryWorkspace.tsx"), "utf8");
if (!/!\s*readOnly \? \(/.test(workspace)) {
  fail("read-only", "LibraryWorkspace does not branch rendering on readOnly — writes would not be disabled");
}

process.stdout.write("ok 10.3: workspace states (unconfigured / read-only / error retry) hold\n");
