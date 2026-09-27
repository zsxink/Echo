#!/usr/bin/env node
// Task 1.3 self-test: verification runners must exit nonzero for unknown IDs,
// missing commands, failing commands, missing human evidence, or a selected
// test command that ran zero tests; they must never modify lockfiles.
//
// It drives the real runner against a temporary fixture manifest and asserts
// each failure mode plus one passing case, then checks the repo lockfiles are
// byte-for-byte unchanged after a full run.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const RUNNER = resolve(ROOT, "scripts", "verify", "run-task.mjs");
const SCENARIO_RUNNER = resolve(ROOT, "scripts", "verify", "run-scenario.mjs");
const LOCKFILES = ["Cargo.lock", "pnpm-lock.yaml"].map((f) =>
  resolve(ROOT, f),
);

let failures = 0;
function assert(cond, label) {
  if (cond) {
    process.stdout.write(`ok   ${label}\n`);
  } else {
    process.stderr.write(`FAIL ${label}\n`);
    failures += 1;
  }
}

function hashOf(path) {
  try {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
  } catch {
    return null;
  }
}

function runRunner(manifestPath, args) {
  return spawnSync(process.execPath, [RUNNER, ...args], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ECHO_VERIFY_MANIFEST: manifestPath },
  });
}

function runScenarioRunner(manifestPath, args, env = {}) {
  return spawnSync(process.execPath, [SCENARIO_RUNNER, ...args], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ...env, ECHO_VERIFY_MANIFEST: manifestPath },
  });
}

const dir = mkdtempSync(join(tmpdir(), "echo-verify-selftest-"));
const fixture = resolve(dir, "manifest.json");
writeFileSync(
  fixture,
  JSON.stringify({
    version: 1,
    tasks: [
      { id: "ok", title: "passing", commands: [{ desc: "x", cmd: "node -e 'process.exit(0)'" }] },
      { id: "missing-command", title: "no command" },
      { id: "failing", title: "failing", commands: [{ desc: "x", cmd: "node -e 'process.exit(3)'" }] },
      {
        id: "missing-evidence",
        title: "missing evidence",
        commands: [{ desc: "x", cmd: "node -e 'process.exit(0)'" }],
        evidence: [{ name: "native.txt", file: "./nonexistent/native.txt" }],
      },
    ],
    scenarios: [],
  }),
  "utf8",
);

const scenarioFixture = resolve(dir, "scenario-manifest.json");
const fakeBin = resolve(dir, "bin");
mkdirSync(fakeBin);
const fakeCargo = resolve(fakeBin, "cargo");
writeFileSync(
  fakeCargo,
  "#!/bin/sh\ncase \"$*\" in\n  *one-filter*) name=one ;;\n  *two-filter*) name=two ;;\n  *) name=other ;;\nesac\nif [ -n \"$ECHO_FAKE_CARGO_MARKERS\" ] && [ \"$name\" != other ]; then\n  touch \"$ECHO_FAKE_CARGO_MARKERS/$name.started\"\n  other=one; [ \"$name\" = one ] && other=two\n  i=0\n  while [ ! -e \"$ECHO_FAKE_CARGO_MARKERS/$other.started\" ] && [ $i -lt 40 ]; do sleep 0.05; i=$((i + 1)); done\n  [ -e \"$ECHO_FAKE_CARGO_MARKERS/$other.started\" ] || exit 9\nfi\nif [ \"$name\" = other ]; then\n  printf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n'\nelse\n  printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n'\nfi\n",
  "utf8",
);
chmodSync(fakeCargo, 0o755);
writeFileSync(
  scenarioFixture,
  JSON.stringify({
    version: 1,
    tasks: [],
    scenarios: [
      {
        id: "zero-tests",
        title: "zero selected tests",
        command: "cargo test -- impossible-filter",
      },
      {
        id: "one-test",
        title: "one selected test",
        command: "cargo test -- one-filter",
      },
      {
        id: "one-test-duplicate",
        title: "same selected test in another scenario",
        command: "cargo test -- one-filter",
      },
      {
        id: "two-test",
        title: "parallel selected test",
        command: "cargo test -- two-filter",
      },
    ],
  }),
  "utf8",
);

// Failure modes must return nonzero.
const cases = [
  [["bogus-id"], "unknown id is nonzero"],
  [["missing-command"], "missing command is nonzero"],
  [["failing"], "failing command is nonzero"],
  [["missing-evidence"], "missing human evidence is nonzero"],
  [["ok", "bogus-id"], "partial failure is nonzero"],
];
for (const [args, label] of cases) {
  const r = runRunner(fixture, args);
  assert(r.status !== 0, `${label} (got ${r.status})`);
}

// Successful single task returns zero.
const okRun = runRunner(fixture, ["ok"]);
assert(okRun.status === 0, "passing task is zero");

// A selected Cargo command that produces a successful zero-test result must
// still fail; a nonzero count proves the normal path remains accepted.
const scenarioEnv = { PATH: `${fakeBin}:${process.env.PATH || ""}` };
const zeroScenario = runScenarioRunner(scenarioFixture, ["zero-tests"], scenarioEnv);
assert(zeroScenario.status !== 0, "zero selected tests is nonzero");
const oneScenario = runScenarioRunner(scenarioFixture, ["one-test"], scenarioEnv);
assert(oneScenario.status === 0, "selected test count above zero is accepted");
const duplicateScenarios = runScenarioRunner(
  scenarioFixture,
  ["one-test", "one-test-duplicate"],
  scenarioEnv,
);
assert(
  duplicateScenarios.status === 0 && /scenario batches: 1 commands for 2 scenarios/.test(duplicateScenarios.stdout),
  "duplicate scenario commands execute in one batch",
);
const markerDir = resolve(dir, "parallel-markers");
mkdirSync(markerDir);
const parallelScenarios = runScenarioRunner(
  scenarioFixture,
  ["one-test", "two-test"],
  { ...scenarioEnv, ECHO_VERIFY_TEST_CONCURRENCY: "2", ECHO_FAKE_CARGO_MARKERS: markerDir },
);
assert(
  parallelScenarios.status === 0
    && /scenario batches: 2 commands for 2 scenarios/.test(parallelScenarios.stdout),
  "independent test commands can execute concurrently",
);

// Lockfiles must be unchanged after a run.
const before = LOCKFILES.map(hashOf);
runRunner(fixture, ["ok", "failing", "--all"]);
const after = LOCKFILES.map(hashOf);
assert(
  JSON.stringify(before) === JSON.stringify(after),
  "lockfiles unchanged by verify run",
);

rmSync(dir, { recursive: true, force: true });

if (failures) {
  process.stderr.write(`self-test: ${failures} failure(s)\n`);
  process.exit(1);
}
process.stdout.write("self-test: all task-verifier assertions pass\n");
