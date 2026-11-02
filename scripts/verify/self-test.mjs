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

function runRunner(manifestPath, args, env = {}) {
  return spawnSync(process.execPath, [RUNNER, ...args], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ...env, ECHO_VERIFY_MANIFEST: manifestPath },
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
      {
        id: "multi-name",
        title: "one cargo start asserts several test names",
        commands: [{
          desc: "x",
          cmd: "node scripts/verify/checks/task-4.mjs multi-name application::import::logic::tests::alpha application::import::logic::tests::beta",
        }],
      },
      {
        id: "missing-name",
        title: "a declared test name the cargo output never reports",
        commands: [{
          desc: "x",
          cmd: "node scripts/verify/checks/task-4.mjs missing-name domain::text::platform_golden_cases::gamma domain::text::platform_golden_cases::delta",
        }],
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
const fakePnpm = resolve(fakeBin, "pnpm");
writeFileSync(
  fakeCargo,
  "#!/bin/sh\n" +
    "name=\n" +
    "case \"$*\" in\n" +
    "  *application::import*)\n" +
    "    printf 'test application::import::logic::tests::alpha ... ok\\n'\n" +
    "    printf 'test application::import::logic::tests::beta ... ok\\n'\n" +
    "    passed=2 ;;\n" +
    "  *domain::text*)\n" +
    "    printf 'test domain::text::platform_golden_cases::gamma ... ok\\n'\n" +
    "    passed=1 ;;\n" +
    "  *one-filter*) printf 'test one_filter_ok ... ok\\n'; name=one; passed=1 ;;\n" +
    "  *two-filter*) printf 'test two_filter_ok ... ok\\n'; name=two; passed=1 ;;\n" +
    "  *perf-filter*) printf 'test perf_filter_ok ... ok\\n'; passed=1 ;;\n" +
    "  *) passed=0 ;;\n" +
    "esac\n" +
    "if [ -n \"$ECHO_FAKE_CARGO_LOG\" ]; then printf '%s\\n' \"$*\" >> \"$ECHO_FAKE_CARGO_LOG\"; fi\n" +
    "if [ -n \"$ECHO_FAKE_CARGO_MARKERS\" ] && [ -n \"$name\" ]; then\n" +
    "  touch \"$ECHO_FAKE_CARGO_MARKERS/$name.started\"\n" +
    "  other=one; [ \"$name\" = one ] && other=two\n" +
    "  i=0\n" +
    "  while [ ! -e \"$ECHO_FAKE_CARGO_MARKERS/$other.started\" ] && [ $i -lt 40 ]; do sleep 0.05; i=$((i + 1)); done\n" +
    "  [ -e \"$ECHO_FAKE_CARGO_MARKERS/$other.started\" ] || exit 9\n" +
    "fi\n" +
    "printf 'test result: ok. %s passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n' \"$passed\"\n",
  "utf8",
);
chmodSync(fakeCargo, 0o755);
writeFileSync(
  fakePnpm,
  "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ECHO_FAKE_PNPM_MARKERS\"\n" +
    "printf ' \\u2713 src/example.test.tsx > first case\\n \\u2713 src/example.test.tsx > second case\\nTests 2 passed\\n'\n",
  "utf8",
);
chmodSync(fakePnpm, 0o755);
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
        evidence: [{ kind: "rust", target: "never_runs" }],
      },
      {
        id: "one-test",
        title: "one selected test",
        command: "cargo test -- one-filter",
        evidence: [{ kind: "rust", target: "one_filter_ok" }],
      },
      {
        id: "one-test-duplicate",
        title: "same selected test in another scenario",
        command: "cargo test -- one-filter",
        evidence: [{ kind: "rust", target: "one_filter_ok" }],
      },
      {
        id: "two-test",
        title: "parallel selected test",
        command: "cargo test -- two-filter",
        evidence: [{ kind: "rust", target: "two_filter_ok" }],
      },
      {
        id: "rust-reused-test",
        title: "Rust test explicitly delegated to the governance job",
        command: "cargo test -p echo-core --all-features regular-filter",
        delegatedTo: "governance",
        evidence: [{ kind: "rust", target: "regular_test_ok" }],
      },
      {
        id: "rust-perf-test",
        title: "performance test explicitly delegated to the perf job",
        command: "cargo test -p echo-core --all-features perf-filter",
        delegatedTo: "perf",
        evidence: [{ kind: "rust", target: "perf_filter_ok" }],
      },
      {
        id: "rust-ignored-test",
        title: "ignored performance test must still execute",
        command: "cargo test -p echo-core --all-features ignored-filter -- --ignored",
        evidence: [{ kind: "rust", target: "ignored_test_ok" }],
      },
      {
        id: "ui-one-test",
        title: "one focused UI test",
        command: "pnpm --filter @echo/desktop test -- --run src/example.test.tsx -t \"first case\"",
        evidence: [{ kind: "vitest", target: "src/example.test.tsx", name: "first case" }],
      },
      {
        id: "ui-two-test",
        title: "another focused UI test in the same file",
        command: "pnpm --filter @echo/desktop test -- --run src/example.test.tsx -t \"second case\"",
        evidence: [{ kind: "vitest", target: "src/example.test.tsx", name: "second case" }],
      },
      {
        id: "unobserved-evidence",
        title: "evidence the aggregated command's output never shows",
        command: "cargo test -- one-filter",
        evidence: [{ kind: "rust", target: "never_printed_test" }],
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

// One cargo invocation can prove several declared test names; a name the
// output never reports must fail and be named in the diagnostic.
const taskEnv = { PATH: `${fakeBin}:${process.env.PATH || ""}` };
const cargoLog = resolve(dir, "cargo-invocations.txt");
const multiName = runRunner(fixture, ["multi-name"], { ...taskEnv, ECHO_FAKE_CARGO_LOG: cargoLog });
assert(
  multiName.status === 0 && readFileSync(cargoLog, "utf8").trim().split("\n").length === 1,
  "one cargo start asserts several test names",
);
const missingName = runRunner(fixture, ["missing-name"], taskEnv);
assert(
  missingName.status !== 0 && missingName.stderr.includes("delta"),
  "a declared test name missing from cargo output fails and names it",
);

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
const unobservedEvidence = runScenarioRunner(scenarioFixture, ["unobserved-evidence"], scenarioEnv);
assert(
  unobservedEvidence.status !== 0 && unobservedEvidence.stderr.includes("never_printed_test"),
  "declared evidence missing from the command output fails and names the target",
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
const pnpmMarkers = resolve(dir, "pnpm-commands.txt");
const groupedReactScenarios = runScenarioRunner(
  scenarioFixture,
  ["ui-one-test", "ui-two-test"],
  { ...scenarioEnv, ECHO_FAKE_PNPM_MARKERS: pnpmMarkers },
);
assert(
  groupedReactScenarios.status === 0
    && /scenario batches: 1 commands for 2 scenarios/.test(groupedReactScenarios.stdout)
    && readFileSync(pnpmMarkers, "utf8").trim().split("\n").length === 1,
  "focused UI scenarios reuse one complete frontend test-suite execution",
);
const reusedRustScenarios = runScenarioRunner(
  scenarioFixture,
  ["--delegated=governance", "rust-reused-test", "rust-ignored-test"],
  scenarioEnv,
);
assert(
  reusedRustScenarios.status !== 0
    && reusedRustScenarios.stdout.includes("ok: scenario rust-reused-test")
    && reusedRustScenarios.stdout.includes("covered by the governance job")
    && reusedRustScenarios.stderr.includes("rust-ignored-test"),
  "only explicitly delegated scenarios are skipped; undeclared Rust scenarios still execute",
);
const multiGateScenarios = runScenarioRunner(
  scenarioFixture,
  ["--delegated=governance,perf", "rust-reused-test", "rust-perf-test"],
  scenarioEnv,
);
assert(
  multiGateScenarios.status === 0
    && multiGateScenarios.stdout.includes("covered by the governance job")
    && multiGateScenarios.stdout.includes("covered by the perf job"),
  "comma-separated delegated gates cover both scenarios",
);
const singlePerfGate = runScenarioRunner(
  scenarioFixture,
  ["--delegated=perf", "rust-reused-test", "rust-perf-test"],
  scenarioEnv,
);
assert(
  singlePerfGate.status !== 0
    && singlePerfGate.stdout.includes("covered by the perf job")
    && singlePerfGate.stderr.includes("rust-reused-test"),
  "a single delegated gate covers only its declared scenarios",
);

// Lockfiles must be unchanged after a run.
const before = LOCKFILES.map(hashOf);
runRunner(fixture, ["ok", "failing", "--all"], taskEnv);
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
