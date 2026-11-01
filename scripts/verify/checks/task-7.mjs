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

// `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
// or moved test would leave this gate green while proving nothing. Require at
// least one test to have actually run.
function runTests(args) {
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
    process.stderr.write(`FAIL ${taskId}: filter '${filter}' matched no tests\n${out}`);
    process.exit(1);
  }
}

runTests(["test", "-p", "echo-desktop", "--all-features", filter]);
run("cargo", ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);
process.stdout.write(`ok ${taskId}: ${filter}\n`);