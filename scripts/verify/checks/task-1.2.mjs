#!/usr/bin/env node
// Task 1.2 check: the React + TypeScript + Vite frontend must pass strict
// typecheck, lint, format check, tests and a production build.
//
// Fails (nonzero) unless every step succeeds.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const STEPS = [
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  ["lint", ["--filter", "@echo/desktop", "lint"]],
  ["format:check", ["--filter", "@echo/desktop", "format:check"]],
  ["test", ["--filter", "@echo/desktop", "test", "--", "--run"]],
  ["build", ["--filter", "@echo/desktop", "build"]],
];

for (const [name, args] of STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 1.2: '${name}' exited ${r.status}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write("ok 1.2: typecheck, lint, format, test and build pass\n");
