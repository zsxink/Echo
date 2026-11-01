#!/usr/bin/env node
// Task 1.5 check: three-platform CI matrix declares the Rust fmt/clippy/test and
// the frontend format/lint/typecheck/test/build quality commands.
//
// Fails (nonzero) unless `.github/workflows/ci.yml` structurally contains the
// three platform/runner pairs, platform-based names, the `rust` job steps (fmt,
// clippy `-D warnings`, test) and the `frontend` job steps (`pnpm install
// --frozen-lockfile`, format/lint/typecheck/test/build).
//
// The commands themselves run in the Rust/frontend CI matrices and, locally, in
// the governance gate's `quality-core` segment. This check deliberately does not
// re-run them: a task check proves its own behavior, not the shared static gates.

import { readFileSync } from "node:fs";
import { assertPlatformMatrix, assertPlatformName, matrixEntries, parseWorkflow } from "../workflow-matrix.mjs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 1.5: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Structural check: parse the workflow and verify each platform-to-runner pair,
// display-name expression, and exact quality commands CI runs.
// ---------------------------------------------------------------------------
const ciPath = resolve(ROOT, ".github", "workflows", "ci.yml");
let ci;
try {
  ci = readFileSync(ciPath, "utf8");
} catch {
  fail(`cannot read ${ciPath}`);
}

let workflow;
try {
  workflow = parseWorkflow(ci, "ci.yml");
  assertPlatformMatrix(
    matrixEntries(workflow, "rust", "ci.yml"),
    { Linux: { os: "ubuntu-latest" }, macOS: { os: "xcode-27" }, Windows: { os: "windows-latest" } },
    "ci.yml rust",
  );
  assertPlatformMatrix(
    matrixEntries(workflow, "frontend", "ci.yml"),
    { Linux: { os: "ubuntu-latest" }, macOS: { os: "xcode-27" }, Windows: { os: "windows-latest" } },
    "ci.yml frontend",
  );
  assertPlatformName(workflow.jobs.rust.name, "Rust (${{ matrix.platform }})", "ci.yml rust name");
  assertPlatformName(workflow.jobs.frontend.name, "Frontend (${{ matrix.platform }})", "ci.yml frontend name");
} catch (error) {
  fail(error.message);
}

const REQUIRED = [
  "cargo fmt --all -- --check",
  "cargo clippy --workspace --all-targets --all-features -- -D warnings",
  "cargo test --workspace --all-features",
  "pnpm install --frozen-lockfile",
  "pnpm --filter @echo/desktop format:check",
  "pnpm --filter @echo/desktop lint",
  "pnpm --filter @echo/desktop typecheck",
  "pnpm --filter @echo/desktop test -- --run",
  "pnpm --filter @echo/desktop build",
];

for (const needle of REQUIRED) {
  if (!ci.includes(needle)) {
    fail(`ci.yml is missing "${needle}"`);
  }
}

process.stdout.write(
  "ok 1.5: three-platform CI matrix declares the Rust + frontend quality commands (executed by the CI matrices and quality-core)\n",
);