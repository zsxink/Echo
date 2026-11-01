#!/usr/bin/env node
// Phase 5 task verifier: safe import, deletion and crash recovery. Each task
// selects the integration/unit tests that prove its own behavior, then runs
// the shared Core format/lint gates.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const [taskId, testName] = process.argv.slice(2);
if (!taskId || !testName) {
  process.stderr.write("usage: task-5.mjs <task-id> <rust-test-name>\n");
  process.exit(2);
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(`FAIL ${taskId}: ${command} ${args.join(" ")}\n${result.stdout}\n${result.stderr}`);
    process.exit(1);
  }
}

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// or moved test would leave this gate green while proving nothing. Require at
// least one test to have actually run.
function runTests(args, testName) {
  const result = spawnSync("cargo", args, { cwd: ROOT, encoding: "utf8" });
  const out = `${result.stdout || ""}${result.stderr || ""}`;
  if (result.status !== 0) {
    process.stderr.write(`FAIL ${taskId}: cargo ${args.join(" ")}\n${out}`);
    process.exit(1);
  }
  const passed = [...out.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
    (total, match) => total + Number(match[1]),
    0,
  );
  if (passed === 0) {
    process.stderr.write(`FAIL ${taskId}: filter '${testName}' matched no tests\n${out}`);
    process.exit(1);
  }
}

runTests(["test", "-p", "echo-core", "--all-features", testName], testName);
run("cargo", ["clippy", "-p", "echo-core", "--all-targets", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);
process.stdout.write(`ok ${taskId}: ${testName}\n`);
