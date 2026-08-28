#!/usr/bin/env node
// SQLite task verifier. Each task selects the integration test that proves
// its own behavior, then runs the shared Core format/lint gates.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const [taskId, testName] = process.argv.slice(2);
if (!taskId || !testName) {
  process.stderr.write("usage: task-3.mjs <task-id> <sqlite-test-name>\n");
  process.exit(2);
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(`FAIL ${taskId}: ${command} ${args.join(" ")}\n${result.stdout}\n${result.stderr}`);
    process.exit(1);
  }
}

run("cargo", ["test", "-p", "echo-core", `infrastructure::sqlite::tests::${testName}`]);
run("cargo", ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);
process.stdout.write(`ok ${taskId}: SQLite ${testName}\n`);
