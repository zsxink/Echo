#!/usr/bin/env node
// Unified scenario verification runner.
//
// Usage:
//   pnpm verify:scenario -- <scenario-id...>   run the listed scenarios
//   pnpm verify:scenario -- --all              run every registered scenario
//   pnpm verify:scenario -- --automated        run only scenarios with an
//                                               executable automated command
//   pnpm verify:scenario -- --list             print scenario ids from the manifest
//
// Scenarios are resolved/validated against scripts/verify/manifest.json's
// `scenarios` section and executed against the test manifest. The scenario set
// is reconciled with the specs' Scenario IDs at release time (task 13.9);
// here we fail fast on unknown, missing-command or failing scenarios.
//
// The runner never modifies lockfiles.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const MANIFEST = process.env.ECHO_VERIFY_MANIFEST || resolve(ROOT, "scripts", "verify", "manifest.json");

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exitCode = 1;
}

const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const scenarios = manifest.scenarios || [];

function selectedTestCommand(command) {
  return /\bcargo\s+test\b/.test(command) || /\bpnpm\b.*\btest\s+--\s+--run\b/.test(command);
}

function executedTestCount(rawOutput) {
  // Vitest colours its summary when it detects a TTY / `CI` (the runner runs
  // tests too, but the checks here must not, so strip ANSI before counting).
  const output = rawOutput.replace(/\u001B\[[0-9;]*[A-Za-z]/g, "");
  let count = 0;
  // Cargo emits one result line per test binary. A filter that matches nothing
  // exits successfully, but every result line reports `0 passed`.
  for (const match of output.matchAll(/test result: .*?\b(\d+) passed;/g)) {
    count += Number(match[1]);
  }
  // Vitest's summary uses `Tests  N passed` (with flexible terminal spacing).
  for (const match of output.matchAll(/\bTests\s+(\d+)\s+passed\b/g)) {
    count += Number(match[1]);
  }
  return count;
}

function runScenario(id, resultByCommand) {
  const sc = scenarios.find((s) => s.id === id);
  if (!sc) {
    fail(`unknown scenario id: ${id}`);
    return false;
  }
  if (!sc.command || !sc.command.trim()) {
    fail(`scenario ${id}: no verification command registered`);
    return false;
  }
  let outcome = resultByCommand.get(sc.command);
  if (!outcome) {
    const result = spawnSync(sc.command, {
      cwd: ROOT,
      shell: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    outcome = { status: result.status, output: `${result.stdout || ""}${result.stderr || ""}` };
    if (outcome.status === 0 && selectedTestCommand(sc.command)) {
      outcome.count = executedTestCount(outcome.output);
      if (outcome.count === 0) outcome.status = 1;
    }
    resultByCommand.set(sc.command, outcome);
  }
  if (outcome.status !== 0) {
    if (selectedTestCommand(sc.command) && outcome.count === 0) {
      fail(`scenario ${id}: selected test command ran zero tests: ${sc.command}`);
    } else {
      fail(`scenario ${id}: command exited ${outcome.status}: ${sc.command}`);
    }
    return false;
  }
  process.stdout.write(`ok: scenario ${id} (${sc.title})\n`);
  return true;
}

const argv = process.argv.slice(2);

if (argv.includes("--list")) {
  for (const s of scenarios) process.stdout.write(`${s.id}\n`);
  process.exit(0);
}

let requested;
if (argv.includes("--all")) {
  requested = scenarios.map((s) => s.id);
} else if (argv.includes("--automated")) {
  requested = scenarios
    .filter((s) => !/check-native-attestation\.mjs/.test(s.command))
    .map((s) => s.id);
} else {
  const ids = argv.flatMap((a) => (["--all", "--automated"].includes(a) ? [] : a.split(/\s+/)));
  requested = ids.filter(Boolean);
}
if (!requested.length) {
  fail("no scenario ids given (use -- <id...> or -- --all)");
  process.exit(1);
}

let ok = true;
const resultByCommand = new Map();
for (const id of requested) {
  if (!runScenario(id, resultByCommand)) ok = false;
}
process.stdout.write(`scenario batches: ${resultByCommand.size} commands for ${requested.length} scenarios\n`);
process.exit(ok ? 0 : 1);
