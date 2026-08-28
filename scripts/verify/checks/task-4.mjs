#!/usr/bin/env node
// Phase 4 task verifier: media parsing, library scan and watching. Each task
// selects the integration/unit tests that prove its own behavior, then runs
// the shared Core format/lint gates.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const [taskId, testName] = process.argv.slice(2);
if (!taskId || !testName) {
  process.stderr.write("usage: task-4.mjs <task-id> <rust-test-name>\n");
  process.exit(2);
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(`FAIL ${taskId}: ${command} ${args.join(" ")}\n${result.stdout}\n${result.stderr}`);
    process.exit(1);
  }
}

run("cargo", ["test", "-p", "echo-core", "--all-features", testName]);
run("cargo", ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);
process.stdout.write(`ok ${taskId}: ${testName}\n`);
