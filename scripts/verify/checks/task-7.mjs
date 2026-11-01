#!/usr/bin/env node
// Phase 7 task verifier: desktop runtime, Tauri IPC, native preferences and
// security hardening. Each task names all of its tests; they are grouped by the
// first two module segments so one cargo invocation proves a whole module.
// Every declared name must appear in a `test <name> ... ok` line, and every
// group must execute at least one test — a renamed or moved test can never
// leave this gate green.
//
// The workspace format/lint gates live in the quality-core governance segment;
// this check only makes the task's own targeted assertions.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const [taskId, ...testNames] = process.argv.slice(2);
if (!taskId || testNames.length === 0) {
  process.stderr.write("usage: task-7.mjs <task-id> <rust-test-name>...\n");
  process.exit(2);
}

function fail(msg) {
  process.stderr.write(`FAIL ${taskId}: ${msg}\n`);
  process.exit(1);
}

// First two module segments identify the module the tests live in; a bare test
// name is its own group.
function groupOf(name) {
  const segments = name.split("::");
  return segments.length >= 2 ? segments.slice(0, 2).join("::") : name;
}

function runGroup(group, names) {
  const result = spawnSync("cargo", ["test", "-p", "echo-desktop", "--all-features", group], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const out = `${result.stdout || ""}${result.stderr || ""}`;
  if (result.status !== 0) {
    fail(`cargo test -p echo-desktop --all-features ${group}\n${out}`);
  }
  const okLines = out.split("\n").filter((line) => /^test .+ \.\.\. ok$/.test(line));
  const passed = [...out.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
    (total, match) => total + Number(match[1]),
    0,
  );
  if (passed === 0) {
    fail(`filter '${group}' matched no tests\n${out}`);
  }
  const missing = names.filter((name) => !okLines.some((line) => line.includes(name)));
  if (missing.length) {
    fail(`declared test(s) not reported as ok under '${group}': ${missing.join(", ")}\n${out}`);
  }
}

const groups = new Map();
for (const name of testNames) {
  const group = groupOf(name);
  if (!groups.has(group)) groups.set(group, []);
  groups.get(group).push(name);
}
for (const [group, names] of groups) {
  runGroup(group, names);
}
process.stdout.write(`ok ${taskId}: ${testNames.length} test(s) in ${groups.size} module group(s)\n`);
