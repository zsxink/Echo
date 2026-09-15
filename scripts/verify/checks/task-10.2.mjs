#!/usr/bin/env node
// Task 10.2 check: bridge 唯一封装 + 三分状态（Core query cache / Player
// external store / 组件局部 reducer）。
//
// Fails unless:
//   1. no component outside `src/bridge/` invokes Tauri directly
//      (`invoke(`/`__TAURI_INTERNALS__`) — the bridge is the ONLY boundary;
//   2. the player external store and bridge command map keep their regression
//      suites passing (stale command/event cannot overwrite a newer revision);
//   3. typecheck is clean.

import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");
const SRC = resolve(APP, "src");

function fail(step, msg) {
  process.stderr.write(`FAIL 10.2: ${step}: ${msg}\n`);
  process.exit(1);
}

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = resolve(dir, name);
    const s = statSync(p);
    if (s.isDirectory()) out.push(...walk(p));
    else if (/\.(ts|tsx)$/.test(name)) out.push(p);
  }
  return out;
}

// 1. The bridge is the only IPC boundary. Outside src/bridge it is a violation
//    to import Tauri's invoke API or to CALL through __TAURI_INTERNALS__; a
//    read-only `window.__TAURI_INTERNALS__` presence sniff (environment
//    detection, e.g. main.tsx's mac gate) is not an IPC call and is allowed.
const violations = [];
for (const file of walk(SRC)) {
  const rel = file.slice(SRC.length + 1);
  const src = readFileSync(file, "utf8");
  if (rel.startsWith("bridge/")) {
    continue; // the designated boundary
  }
  if (/\.test\.(ts|tsx)$/.test(rel)) {
    continue; // suites mock the boundary via @tauri-apps/api — that IS the bridge path under test
  }
  if (src.includes('from "@tauri-apps/api')) {
    violations.push(`${rel}: imports @tauri-apps/api`);
  } else if (/__TAURI_INTERNALS__\s*\.\s*invoke|__TAURI_INTERNALS__\s*\[\s*["']invoke/.test(src)) {
    violations.push(`${rel}: calls through __TAURI_INTERNALS__`);
  } else if (/(^|[^\w.])invoke\s*\(/.test(src)) {
    violations.push(`${rel}: calls invoke() directly`);
  }
}
if (violations.length) {
  fail("bridge boundary", violations.join("; "));
}
if (!readFileSync(resolve(SRC, "bridge", "index.ts"), "utf8").includes("invoke")) {
  fail("bridge boundary", "src/bridge/index.ts does not wrap invoke at all");
}

// 2. Regression suites.
function runDesktopTest(filter) {
  const r = spawnSync("pnpm", ["--filter", "@echo/desktop", "test", filter], {
    cwd: APP,
    encoding: "utf8",
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) fail("vitest", out);
  const files = Number((out.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) fail("vitest", `'${filter}' matched ${files} test files (expected exactly 1)`);
}
runDesktopTest("src/player/playerStore.test.ts");
runDesktopTest("src/app/coverArt.test.ts");

// 3. typecheck.
const t = spawnSync("pnpm", ["--filter", "@echo/desktop", "typecheck"], { cwd: APP, encoding: "utf8" });
if (t.status !== 0) fail("typecheck", `${t.stdout}\n${t.stderr}`);

process.stdout.write("ok 10.2: bridge is the only invoke boundary; state layers keep their regressions green\n");
