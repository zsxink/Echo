#!/usr/bin/env node
// Task 1.8 check: unified error logging + local diagnostics-directory
// convention (docs/LOGGING.md) with privacy tests on both sides.
//
// Fails (nonzero) unless:
//   1. docs/LOGGING.md exists and documents the privacy keywords (absolute
//      path redaction, lyrics/tag exclusion).
//   2. `cargo test -p echo-core` passes, which includes crates/echo-core/tests
//      /logging.rs proving default test logs carry no full absolute paths,
//      lyrics, tag text or file contents.
//   3. the frontend privacy test passes:
//      `pnpm --filter @echo/desktop test -- --run src/logging`.
//   4. `cargo clippy -p echo-core --all-targets -- -D warnings` and
//      `cargo fmt --all -- --check` pass (workspace stays lint-clean).
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
const rustTest = spawnSync("cargo", ["test", "-p", "echo-core"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (rustTest.status !== 0) {
  process.stderr.write(
    `FAIL 1.8: cargo test -p echo-core exited ${rustTest.status}\n${rustTest.stdout}\n${rustTest.stderr}\n`,
  );
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

// ---------------------------------------------------------------------------
// Step 4: workspace stays lint-clean (clippy + fmt on the core).
// ---------------------------------------------------------------------------
const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) {
  process.stderr.write(
    `FAIL 1.8: cargo clippy -p echo-core --all-targets -- -D warnings exited ${clippy.status}\n${clippy.stdout}\n${clippy.stderr}\n`,
  );
  process.exit(1);
}

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (fmt.status !== 0) {
  process.stderr.write(
    `FAIL 1.8: cargo fmt --all -- --check exited ${fmt.status}\n${fmt.stdout}\n${fmt.stderr}\n`,
  );
  process.exit(1);
}

process.stdout.write("ok 1.8: unified error logging convention with privacy tests\n");