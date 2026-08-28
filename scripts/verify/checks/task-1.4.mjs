#!/usr/bin/env node
// Task 1.4 check: module visibility rules and architecture tests.
//
// Fails unless `echo-core`'s arch test suite passes: it enforces the
// domain/application/infrastructure layering, forbids Tauri/mpv/React deps and
// `use`s in echo-core, and rejects platform `cfg` business branches. The suite
// itself contains "can reject" tests proving violations are caught.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

const r = spawnSync("cargo", ["test", "-p", "echo-core", "--test", "arch"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (r.status !== 0) {
  process.stderr.write(`FAIL 1.4: echo-core arch tests exited ${r.status}\n${r.stdout}\n${r.stderr}\n`);
  process.exit(1);
}

process.stdout.write("ok 1.4: echo-core architecture tests pass\n");
