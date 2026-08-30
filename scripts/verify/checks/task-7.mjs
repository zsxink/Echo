#!/usr/bin/env node
// Phase 7 task verifier: desktop runtime, Tauri IPC, native preferences and
// security hardening. Each task selects the echo-desktop integration/unit tests
// that prove its own behavior, then runs the shared desktop format/lint gates.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const [taskId, filter] = process.argv.slice(2);
if (!taskId || !filter) {
  process.stderr.write("usage: task-7.mjs <task-id> <cargo-test-filter>\n");
  process.exit(2);
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(
      `FAIL ${taskId}: ${command} ${args.join(" ")}\n${result.stdout}\n${result.stderr}`,
    );
    process.exit(1);
  }
}

run("cargo", ["test", "-p", "echo-desktop", "--all-features", filter]);
run("cargo", ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);
process.stdout.write(`ok ${taskId}: ${filter}\n`);