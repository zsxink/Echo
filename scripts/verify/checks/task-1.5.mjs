#!/usr/bin/env node
// Task 1.5 check: three-platform CI matrix runs Rust fmt/clippy/test and the
// frontend format/lint/typecheck/test/build — and the same quality commands
// pass on the current machine.
//
// Fails (nonzero) unless `.github/workflows/ci.yml` structurally contains the
// three runners (`ubuntu-latest`, `macos-latest`, `windows-latest`), the `rust`
// job steps (fmt, clippy `-D warnings`, test) and the `frontend` job steps
// (`pnpm install --frozen-lockfile`, format/lint/typecheck/test/build), and
// every one of those commands succeeds locally.
//
// Note: the Windows/Linux pipeline runs execute in GitHub-hosted runners and
// cannot run on this macOS machine; the local run below verifies the same
// commands on the current platform and the structural check verifies the
// matrix configuration.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 1.5: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Structural check: the workflow file must contain the matrix runners and the
// exact quality commands CI runs. Read it as a flat document; the strings we
// assert on are what the YAML parses to, independent of formatting.
// ---------------------------------------------------------------------------
const ciPath = resolve(ROOT, ".github", "workflows", "ci.yml");
let ci;
try {
  ci = readFileSync(ciPath, "utf8");
} catch {
  fail(`cannot read ${ciPath}`);
}

const REQUIRED = [
  "ubuntu-latest",
  "macos-latest",
  "windows-latest",
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

// ---------------------------------------------------------------------------
// Local pipeline: the same quality commands CI runs, on this machine. Fail fast
// on the first failing step.
// ---------------------------------------------------------------------------
const RUST_STEPS = [
  [
    "cargo fmt --all -- --check",
    ["cargo", "fmt", "--all", "--", "--check"],
  ],
  [
    "cargo clippy --workspace --all-targets --all-features -- -D warnings",
    ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"],
  ],
  [
    "cargo test --workspace --all-features",
    ["cargo", "test", "--workspace", "--all-features"],
  ],
];

const FRONTEND_STEPS = [
  ["format:check", ["--filter", "@echo/desktop", "format:check"]],
  ["lint", ["--filter", "@echo/desktop", "lint"]],
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  ["test -- --run", ["--filter", "@echo/desktop", "test", "--", "--run"]],
  ["build", ["--filter", "@echo/desktop", "build"]],
];

for (const [name, args] of RUST_STEPS) {
  const r = spawnSync(args[0], args.slice(1), { cwd: ROOT, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 1.5: '${name}' exited ${r.status}\n${r.stdout}\n${r.stderr}\n`);
    process.exit(1);
  }
}

for (const [name, args] of FRONTEND_STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 1.5: frontend '${name}' exited ${r.status}\n${r.stdout}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write(
  "ok 1.5: three-platform CI matrix configured and Rust + frontend quality commands pass on this machine\n",
);