#!/usr/bin/env node
// Task 8.12 check: 用保证格式 fixtures 驱动真实 libmpv 的 smoke 测试。
//
// Fails unless:
//   1. the smoke suite exists (tests/player_smoke.rs) and passes;
//   2. the suite actually covers the fixture matrix: every guaranteed format,
//      a corrupt file, an MP4 without an audio track, seek / track-switch /
//      exit, and backend teardown (resource release);
//   3. the suite is not a silent skip: no `#[ignore]`, and mpv unavailability
//      fails the test rather than returning early (a skip-on-missing-mpv
//      branch would let this gate pass without ever touching libmpv).

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.12: ${step}: ${msg}\n`);
  process.exit(1);
}

const suite = resolve(ROOT, "crates", "echo-desktop", "tests", "player_smoke.rs");
let src;
try {
  src = readFileSync(suite, "utf8");
} catch {
  fail("suite", "tests/player_smoke.rs does not exist");
}

// 3. No silent skips before running anything.
if (src.includes("#[ignore]")) fail("skip guard", "smoke tests must not carry #[ignore]");
const earlyReturn = /fn (ensure|require|skip)[a-z_]*\([^)]*\)[^{]*\{[^}]*return;?\s*\}/s;
if (earlyReturn.test(src)) {
  fail("skip guard", "an early-return skip branch exists — mpv unavailability must fail, not pass");
}

// 1. The suite runs against the REAL vendored libmpv. A bare `cargo test`
//    runner has no Frameworks rpath, and the suite then silently skips — which
//    is exactly the false green this gate exists to prevent. Stage the dylib
//    dependency dir (the same one build.rs stages to target/Frameworks) onto
//    DYLD_LIBRARY_PATH and FAIL if the skip branch ever fires.
const env = { ...process.env };
const frameworks = resolve(ROOT, "target", "Frameworks");
env.DYLD_LIBRARY_PATH = frameworks + (env.DYLD_LIBRARY_PATH ? `:${env.DYLD_LIBRARY_PATH}` : "");
const r = spawnSync(
  "cargo",
  ["test", "-p", "echo-desktop", "--all-features", "--test", "player_smoke", "--", "--nocapture"],
  { cwd: ROOT, encoding: "utf8", env },
);
const out = `${r.stdout || ""}${r.stderr || ""}`;
if (out.includes("skipping real playback smoke")) {
  fail(
    "skip guard",
    `the suite took the skip branch (rpath deps unreachable). DYLD_LIBRARY_PATH was set to ${frameworks}; ` +
      "if that dir lacks the dylibs, re-run the macOS build staging first",
  );
}
if (r.status !== 0) fail("player_smoke", out);
for (const name of ["guaranteed_formats_each_load_via_real_libmpv", "consecutive_loads_then_clean_exit"]) {
  if (!out.includes(`${name} ... ok`)) fail("player_smoke", `test ${name} did not pass`);
}

// 2. Fixture-matrix coverage by test name. The suite drives: every guaranteed
//    format load, a corrupt file (must NOT play), a no-audio MP4, seek inside a
//    real load, a track-change via consecutive loads, and orderly teardown
//    (shutdown releases the backend handle).
for (const name of ["corrupt", "no-audio", "seek", "track-change", "teardown"]) {
  if (!src.toLowerCase().includes(name)) {
    fail("coverage", `smoke suite does not exercise '${name}'`);
  }
}

process.stdout.write("ok 8.12: real libmpv smoke covers the fixture matrix with no silent skips\n");
