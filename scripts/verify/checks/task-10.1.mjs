#!/usr/bin/env node
// Task 10.1 check: 设计 token 落地（三主题 + 语义色不被主题覆盖）。
//
// Fails unless:
//   1. tokens.css defines the three IPC themes (coral / cobalt / turquoise)
//      as `:root[data-echo-theme=…]` blocks with an `--accent`;
//   2. the semantic favorite red (`--danger`) is defined once at `:root` and
//      overridden by NO theme block — the 收藏 red is semantic, not thematic;
//   3. the shell/component suites that render from these tokens pass.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(step, msg) {
  process.stderr.write(`FAIL 10.1: ${step}: ${msg}\n`);
  process.exit(1);
}

const tokens = readFileSync(resolve(APP, "src", "styles", "tokens.css"), "utf8");

// 1. Three themes, each with its own accent.
for (const theme of ["coral", "cobalt", "turquoise"]) {
  const marker = `:root[data-echo-theme="${theme}"]`;
  const at = tokens.indexOf(marker);
  if (at === -1) fail("themes", `tokens.css has no ${theme} theme block`);
  const rest = tokens.slice(at, at + 400);
  if (!rest.includes("--accent:")) {
    fail("themes", `the ${theme} theme block does not define --accent`);
  }
}

// 2. --danger is global and never theme-overridden.
if (!tokens.includes("--danger:")) fail("semantic", "tokens.css never defines --danger");
const themeBlocks = tokens.match(/:root\[data-echo-theme[^\n]*\{\n[\s\S]*?\n\}/g) ?? [];
for (const block of themeBlocks) {
  if (block.includes("--danger:")) {
    fail("semantic", `a theme block overrides the semantic --danger (收藏红不得被主题覆盖):\n${block}`);
  }
}

// 3. The suites that render from tokens still pass.
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

runDesktopTest("src/app/App.test.tsx");
runDesktopTest("src/features/library/SongList.test.tsx");

process.stdout.write("ok 10.1: three themes + semantic favorite red land in the token layer\n");
