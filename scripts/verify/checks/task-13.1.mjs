#!/usr/bin/env node
// Task 13.1 check: mock-bridge browser E2E covering PRD A1–A14 non-platform
// acceptance, run in a real Chromium (headless) via the Chrome DevTools
// Protocol against the *built* app bundle.
//
// This invokes `pnpm test:e2e` (apps/desktop/e2e/run-e2e.mjs) which:
//  - serves the built bundle, installs the mock `__TAURI_INTERNALS__` bridge,
//    and drives the unmodified app through PRD A1/A6/A7/A8/A13/A14 (the
//    non-platform acceptance paths; A2/A3/A4/A5/A9/A10/A11 are covered by the
//    Rust fault/scan/import/recovery matrix + the UI component suites and this
//    check asserts their UI surfaces render where the mock drives them);
//  - exits nonzero on any acceptance failure, and also when a scenario could
//    not run at all (a suite that asserts nothing must not report green).
//
// The bundle is rebuilt here, unconditionally. Driving a *stale* dist would
// exercise code the working tree no longer has and still report success, which
// is the same "ran nothing, passed everything" failure mode as a missing dist.
//
// Requires a Chromium at CHROME_PATH (defaults to the macOS Google Chrome).
// On non-macOS or when no Chrome is present the check fails loudly (browser
// E2E is a hard 13.1 acceptance).

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 13.1: ${msg}\n`);
  process.exit(1);
}

const chrome = process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
if (!existsSync(chrome)) {
  fail(`no Chromium at ${chrome}; set CHROME_PATH (browser E2E is a hard acceptance)`);
}

// Rebuild the renderer first: the E2E must drive the current working tree.
const build = spawnSync("pnpm", ["--filter", "@echo/desktop", "build"], {
  cwd: APP,
  encoding: "utf8",
});
if (build.status !== 0) {
  fail(`pnpm build exited ${build.status}\n${build.stdout || ""}\n${build.stderr || ""}`);
}

const r = spawnSync("pnpm", ["--filter", "@echo/desktop", "test:e2e"], {
  cwd: APP,
  encoding: "utf8",
  env: { ...process.env, CHROME_PATH: chrome },
});
if (r.status !== 0) {
  fail(`pnpm test:e2e exited ${r.status}\n${r.stdout || ""}\n${r.stderr || ""}`);
}

process.stdout.write("ok 13.1: mock-bridge browser E2E (PRD A1–A14 non-platform) passes in real Chromium\n");