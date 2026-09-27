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

import { spawn, spawnSync } from "node:child_process";
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

// Standalone test commands only read the checkout and write isolated test
// output. Keep task scripts serial: some intentionally mutate and restore
// files as part of their proof. Cargo itself coordinates access to target/.
function parallelizableTestCommand(command) {
  return /^\s*cargo\s+test\b/.test(command)
    || /^\s*pnpm\s+--filter\s+@echo\/desktop\s+test\s+--\s+--run\b/.test(command);
}

function scenarioCommand(id) {
  const scenario = scenarios.find((entry) => entry.id === id);
  if (!scenario) {
    fail(`unknown scenario id: ${id}`);
    return null;
  }
  if (!scenario.command || !scenario.command.trim()) {
    fail(`scenario ${id}: no verification command registered`);
    return null;
  }
  return scenario.command;
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
  if (!sc?.command?.trim()) return false;
  const outcome = resultByCommand.get(sc.command);
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

function executeCommand(command, sequence, total) {
  return new Promise((resolveOutcome) => {
    const startedAt = Date.now();
    process.stdout.write(`[${sequence}/${total}] start ${commandLabel(command)}\n`);
    const child = spawn(command, [], {
      cwd: ROOT,
      shell: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", (error) => {
      resolveOutcome({ status: 1, output: error.message, durationMs: Date.now() - startedAt });
    });
    child.on("close", (status) => {
      const output = Buffer.concat([...stdout, ...stderr]).toString("utf8");
      const outcome = { status: status ?? 1, output, durationMs: Date.now() - startedAt };
      if (outcome.status === 0 && selectedTestCommand(command)) {
        outcome.count = executedTestCount(output);
        if (outcome.count === 0) outcome.status = 1;
      }
      resolveOutcome(outcome);
    });
  });
}

function commandLabel(command) {
  return command.length > 120 ? `${command.slice(0, 117)}...` : command;
}

async function executeScenarios(requested) {
  const resultByCommand = new Map();
  const commands = [];
  const seenCommands = new Set();
  for (const id of requested) {
    const command = scenarioCommand(id);
    if (command && !seenCommands.has(command)) {
      commands.push(command);
      seenCommands.add(command);
    }
  }

  const rawConcurrency = Number.parseInt(process.env.ECHO_VERIFY_TEST_CONCURRENCY || "2", 10);
  const concurrency = Number.isFinite(rawConcurrency) && rawConcurrency > 0 ? rawConcurrency : 2;
  const total = commands.length;
  let completed = 0;
  const reuseGovernance = process.env.ECHO_GOVERNANCE_GATE_ALREADY_PASSED === "1";

  for (let index = 0; index < commands.length;) {
    const command = commands[index];
    if (reuseGovernance && command === "node scripts/verify/ci-governance.mjs") {
      resultByCommand.set(command, { status: 0, output: "reused successful Run governance gates step" });
      completed++;
      process.stdout.write(`[${completed}/${total}] reused prior CI governance result\n`);
      index++;
      continue;
    }

    if (parallelizableTestCommand(command)) {
      const batch = [];
      while (index < commands.length && batch.length < concurrency && parallelizableTestCommand(commands[index])) {
        batch.push(commands[index++]);
      }
      const firstSequence = completed + 1;
      const outcomes = await Promise.all(batch.map(async (entry, offset) => {
        const outcome = await executeCommand(entry, firstSequence + offset, total);
        return [entry, outcome];
      }));
      for (const [entry, outcome] of outcomes) {
        resultByCommand.set(entry, outcome);
        completed++;
        const state = outcome.status === 0 ? "ok" : "FAIL";
        process.stdout.write(`[${completed}/${total}] ${state} ${Math.ceil(outcome.durationMs / 1000)}s ${commandLabel(entry)}\n`);
      }
      continue;
    }

    const outcome = await executeCommand(command, completed + 1, total);
    resultByCommand.set(command, outcome);
    completed++;
    const state = outcome.status === 0 ? "ok" : "FAIL";
    process.stdout.write(`[${completed}/${total}] ${state} ${Math.ceil(outcome.durationMs / 1000)}s ${commandLabel(command)}\n`);
    index++;
  }

  let ok = true;
  for (const id of requested) if (!runScenario(id, resultByCommand)) ok = false;
  process.stdout.write(`scenario batches: ${resultByCommand.size} commands for ${requested.length} scenarios\n`);
  return ok;
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

executeScenarios(requested).then((passed) => {
  process.exitCode = passed ? 0 : 1;
}).catch((error) => {
  process.stderr.write(`error: scenario runner failed unexpectedly: ${error.message}\n`);
  process.exitCode = 1;
});
