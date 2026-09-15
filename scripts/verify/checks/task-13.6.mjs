#!/usr/bin/env node
// Task 13.6 check: the complete pre-release quality gate.
//
// Acceptance (tasks 13.6):
//   cargo fmt --all -- --check &&
//   cargo clippy --workspace --all-targets --all-features -- -D warnings &&
//   cargo test --workspace --all-features
// and
//   pnpm format:check && pnpm lint && pnpm typecheck && pnpm test -- --run &&
//   pnpm build
// (all run from the apps/desktop workspace for the frontend half).
//
// Every one of these must pass before any candidate packaging. Fails (nonzero)
// on the first command that fails. This is intentionally the literal set from
// the task — no qualifications, no skips.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 13.6: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 13.6: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

// --- Rust quality gate ---
run("cargo", ["fmt", "--all", "--", "--check"]);
run("cargo", ["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"]);
run("cargo", ["test", "--workspace", "--all-features"]);

// --- Frontend quality gate ---
run("pnpm", ["--filter", "@echo/desktop", "format:check"], APP);
run("pnpm", ["--filter", "@echo/desktop", "lint"], APP);
run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run"], APP);
run("pnpm", ["--filter", "@echo/desktop", "build"], APP);

process.stdout.write("ok 13.6: full Rust + frontend quality gate passes (fmt/clippy/tests + format/lint/typecheck/test/build)\n");