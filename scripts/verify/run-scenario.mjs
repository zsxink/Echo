#!/usr/bin/env node
// Unified scenario verification runner.
//
// Usage:
//   pnpm verify:scenario -- <scenario-id...>   run the listed scenarios
//   pnpm verify:scenario -- --all              run every registered scenario
//   pnpm verify:scenario -- --automated        run only scenarios with an
//                                               executable automated command
//   pnpm verify:scenario -- --delegated=governance <id...>
//                                               skip scenarios whose registry
//                                               entry explicitly declares
//                                               `delegatedTo: "governance"`
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
  return /\bcargo\s+test\b/.test(command) || /\bpnpm\b.*\btest\s+(?:--\s+)?--run\b/.test(command);
}

// Standalone test commands only read the checkout and write isolated test
// output. Keep task scripts serial: some intentionally mutate and restore
// files as part of their proof. Cargo itself coordinates access to target/.
function parallelizableTestCommand(command) {
  return /^\s*cargo\s+test\b/.test(command)
    || /^\s*pnpm\s+--filter\s+@echo\/desktop\s+test\s+(?:--\s+)?--run\b/.test(command);
}

// Scenario mappings can select different files and test names from Vitest.
// Run the complete frontend suite once: the command validator separately
// proves every mapped file/name exists, and the full suite exercises them.
// The verbose reporter is kept so each evidence name is printed and can be
// observed below.
//
// The registry spells the command as `test -- --run` (the historical form);
// pnpm forwards the extra `--` to vitest, which swallows every following
// argument — including a reporter flag. The execution form must drop it:
// `test --run --reporter=verbose` becomes `vitest run --run --reporter=verbose`.
function groupedTestCommand(command) {
  if (/^pnpm --filter @echo\/desktop test -- --run(?:\s|$)/.test(command)) {
    return "pnpm --filter @echo/desktop test --run --reporter=verbose";
  }
  return command;
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

// ANSI colour survives in Vitest summaries and cargo's own colouring; strip it
// before looking for evidence lines.
const stripAnsi = (rawOutput) => rawOutput.replace(/\u001B\[[0-9;]*[A-Za-z]/g, "");

function executedTestCount(rawOutput) {
  // Vitest colours its summary when it detects a TTY / `CI` (the runner runs
  // tests too, but the checks here must not, so strip ANSI before counting).
  const output = stripAnsi(rawOutput);
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

// A scenario's evidence is the contract that replaces the old per-scenario
// test filter: the aggregated command runs a whole domain, so "exited 0" only
// says the domain is green. Each declared target must actually be observed.
//   rust        a `test <name> ... ok` line whose name contains the target
//               (evidence keeps the original command's filter: a module prefix
//               or a bare test name, exactly what the pre-aggregation command
//               selected; the static gates apply the same rule)
//   vitest      the file path appears, and a declared name appears as its own
//               verbose-reporter line
//   node / attestation  the command exit status is the observation
function evidenceObserved(outcome, item) {
  if (item.kind === "node" || item.kind === "attestation") return true;
  const output = stripAnsi(outcome.output || "");
  if (item.kind === "rust") {
    return output.split("\n").some((line) => {
      const match = line.match(/^test (.+?) \.\.\. ok$/);
      return match !== null && match[1].includes(item.target);
    });
  }
  if (item.kind === "vitest") {
    if (!output.includes(item.target)) return false;
    return item.name ? output.includes(item.name) : true;
  }
  return false;
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
  const unobserved = (sc.evidence ?? []).filter((item) => !evidenceObserved(outcome, item));
  if (unobserved.length) {
    for (const item of unobserved) {
      fail(`scenario ${id}: evidence not observed in command output: ${item.kind} '${item.target}'`);
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

async function executeScenarios(requested, delegatedGates) {
  const resultByCommand = new Map();
  const commands = [];
  const seenCommands = new Set();
  const executionByCommand = new Map();
  const covered = new Set();
  for (const id of requested) {
    const scenario = scenarios.find((entry) => entry.id === id);
    // Only a scenario that explicitly declares `delegatedTo: <gate>` can be
    // covered by a job running elsewhere. Never infer coverage from the shape
    // of the command (e.g. "any cargo test is covered"): after aggregation
    // that heuristic silently skipped real work.
    if (scenario && delegatedGates !== null && delegatedGates.has(scenario.delegatedTo)) {
      covered.add(id);
      continue;
    }
    const command = scenarioCommand(id);
    if (command) {
      const executionCommand = groupedTestCommand(command);
      executionByCommand.set(command, executionCommand);
      if (!seenCommands.has(executionCommand)) {
        commands.push(executionCommand);
        seenCommands.add(executionCommand);
      }
    }
  }

  const rawConcurrency = Number.parseInt(process.env.ECHO_VERIFY_TEST_CONCURRENCY || "2", 10);
  const concurrency = Number.isFinite(rawConcurrency) && rawConcurrency > 0 ? rawConcurrency : 2;
  const total = commands.length;
  let completed = 0;

  for (let index = 0; index < commands.length;) {
    const command = commands[index];

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
        if (outcome.status !== 0 && outcome.output.trim()) {
          process.stderr.write(`${outcome.output.slice(-8_000)}\n`);
        }
      }
      continue;
    }

    const outcome = await executeCommand(command, completed + 1, total);
    resultByCommand.set(command, outcome);
    completed++;
    const state = outcome.status === 0 ? "ok" : "FAIL";
    process.stdout.write(`[${completed}/${total}] ${state} ${Math.ceil(outcome.durationMs / 1000)}s ${commandLabel(command)}\n`);
    if (outcome.status !== 0 && outcome.output.trim()) {
      process.stderr.write(`${outcome.output.slice(-8_000)}\n`);
    }
    index++;
  }

  for (const [command, executionCommand] of executionByCommand) {
    resultByCommand.set(command, resultByCommand.get(executionCommand));
  }

  let ok = true;
  for (const id of requested) {
    if (covered.has(id)) {
      const scenario = scenarios.find((entry) => entry.id === id);
      process.stdout.write(`ok: scenario ${id} (${scenario.title}) — covered by the ${scenario.delegatedTo} job\n`);
    } else if (!runScenario(id, resultByCommand)) {
      ok = false;
    }
  }
  process.stdout.write(`scenario batches: ${commands.length} commands for ${requested.length} scenarios (${covered.size} delegated)\n`);
  return ok;
}

const rawArgv = process.argv.slice(2);
const delegatedFlag = rawArgv.find((arg) => arg.startsWith("--delegated="));
const delegatedGates = delegatedFlag
  ? new Set(delegatedFlag.slice("--delegated=".length).split(",").map((s) => s.trim()).filter(Boolean))
  : null;
if (delegatedFlag && (!delegatedGates || delegatedGates.size === 0)) {
  fail("--delegated=<gate> requires at least one gate name");
  process.exit(1);
}
// The flag selects how declared scenarios are covered; it is never a scenario id.
const argv = rawArgv.filter((arg) => !arg.startsWith("--delegated="));

if (argv.includes("--list")) {
  for (const s of scenarios) process.stdout.write(`${s.id}\n`);
  process.exit(0);
}

let requested;
if (argv.includes("--all")) {
  requested = scenarios.map((s) => s.id);
} else if (argv.includes("--automated")) {
  requested = scenarios
    .filter((s) => !(s.evidence ?? []).some((item) => item.kind === "attestation"))
    .map((s) => s.id);
} else {
  const ids = argv.flatMap((a) => (["--all", "--automated"].includes(a) ? [] : a.split(/\s+/)));
  requested = ids.filter(Boolean);
}
if (!requested.length) {
  fail("no scenario ids given (use -- <id...> or -- --all)");
  process.exit(1);
}

executeScenarios(requested, delegatedGates).then((passed) => {
  process.exitCode = passed ? 0 : 1;
}).catch((error) => {
  process.stderr.write(`error: scenario runner failed unexpectedly: ${error.message}\n`);
  process.exitCode = 1;
});
