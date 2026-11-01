#!/usr/bin/env node
// Task 1.8 check: unified error logging + local diagnostics-directory
// convention (docs/LOGGING.md) with privacy tests on both sides.
//
// Fails (nonzero) unless:
//   1. docs/LOGGING.md exists and documents the privacy keywords (absolute
//      path redaction, lyrics/tag exclusion).
//   2. the `logging` integration tests pass and at least one actually ran:
//      crates/echo-core/tests/logging.rs proves default test logs carry no full
//      absolute paths, lyrics, tag text or file contents.
//   3. the frontend privacy test passes:
//      `pnpm --filter @echo/desktop test -- --run src/logging`.
//
// Shared fmt/clippy gates live in the governance gate's quality-core segment.
//
// On success prints: `ok 1.8: unified error logging convention with privacy
// tests`.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 1.8: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Step 1: docs/LOGGING.md exists and covers the privacy policy.
// ---------------------------------------------------------------------------
const docPath = resolve(ROOT, "docs", "LOGGING.md");
let doc;
try {
  doc = readFileSync(docPath, "utf8");
} catch {
  fail(`cannot read ${docPath}`);
}

// The conventions doc is written in Chinese (project documentation language),
// so match the documented Chinese terms rather than forcing English wording.
const requiredKeywords = [
  // Absolute-path redaction must be documented.
  /绝对路径/i,
  /redact|脱敏|净化/i,
  // Lyrics / tag / file-content exclusion must be documented.
  /歌词/i,
  /标签/i,
  /文件内容/i,
];
for (const re of requiredKeywords) {
  if (!re.test(doc)) {
    fail(`docs/LOGGING.md is missing required keyword: ${re}`);
  }
}

// ---------------------------------------------------------------------------
// Step 2: Rust logging privacy tests.
// ---------------------------------------------------------------------------
const rustTest = spawnSync(
  "cargo",
  ["test", "-p", "echo-core", "--all-features", "--test", "logging"],
  { cwd: ROOT, encoding: "utf8" },
);
if (rustTest.status !== 0) {
  process.stderr.write(
    `FAIL 1.8: cargo test -p echo-core --test logging exited ${rustTest.status}\n${rustTest.stdout}\n${rustTest.stderr}\n`,
  );
  process.exit(1);
}
// `cargo test <filter>` exits 0 when the target selects nothing, so a renamed
// or removed integration test must not leave this check green.
const rustPassed = [...`${rustTest.stdout}${rustTest.stderr}`.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
  (total, match) => total + Number(match[1]),
  0,
);
if (rustPassed === 0) {
  process.stderr.write(`FAIL 1.8: logging integration target ran no tests\n${rustTest.stdout}\n${rustTest.stderr}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Step 3: frontend privacy test.
// ---------------------------------------------------------------------------
const feTest = spawnSync(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/logging"],
  { cwd: APP, encoding: "utf8" },
);
if (feTest.status !== 0) {
  process.stderr.write(
    `FAIL 1.8: frontend privacy test exited ${feTest.status}\n${feTest.stdout}\n${feTest.stderr}\n`,
  );
  process.exit(1);
}

process.stdout.write("ok 1.8: unified error logging convention with privacy tests\n");