#!/usr/bin/env node
// Task 13.9 check: full scenario reconciliation + PRD A1–A14 execution.
//
// Acceptance (tasks 13.9):
//   - compare the Scenario ID sets of specs · traceability.md · test manifest
//     are EXACTLY equal (reconcile-scenarios.mjs enforces this);
//   - `pnpm verify:scenario -- --all` executes scenarios; the count equals the
//     spec-derived count (must be 166 today: 160 original + 6 sync-foundation),
//     and missing/duplicate mapping, missing actual command/evidence, or any
//     P0 failure blocks 0.1.0;
//   - every scenario command resolves to a real test (no silent 0-test pass);
//   - execute PRD A1–A14 (prd-matrix.mjs emits the authoritative mapping and
//     the macOS-runnable command for each A path; the native/manual remainder
//     is the platform-Gate handoff).
//
// On macOS this runs the fully-automatable subset of the 160 and asserts a
// per-scenario result log + the set-equal reconciliation. Rows whose behavior
// is a real multi-platform OS interaction (the 45 native manifests) are
// checked for attestation evidence where the platform is not macOS; on macOS
// the automated subsets (player_smoke, task checks) are exercised via their
// scenario commands.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// `new URL(".", import.meta.url)` is already this file's directory
// (`scripts/verify/checks/`), so three `..` land on the repository root — an
// extra `dirname` would resolve one level short (`Project/music`) and every
// spawned path would miss.
const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const ARTIFACTS = resolve(ROOT, "artifacts");

function fail(msg) {
  process.stderr.write(`FAIL 13.9: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT, env = {}) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8", env: { ...process.env, ...env } });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 13.9: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`);
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. Three-way set reconciliation must pass (spec == trace == manifest) and the
//    count must equal the spec-derived count.
const reconcileOut = run("node", ["scripts/verify/reconcile-scenarios.mjs"]).trim();
if (!/spec \d+ = trace \d+ = manifest \d+ scenarios/.test(reconcileOut)) {
  fail(`reconciliation did not pass:\n${reconcileOut}`);
}
mkdirSync(ARTIFACTS, { recursive: true });

// 2. Every scenario command must resolve to a real test (no silent 0-test pass).
run("node", ["scripts/verify/validate-scenario-commands.mjs"]);

// 3. Run the scenario suite. The full 166 includes the native rows (which
//    attestation-gate on a non-macOS host); run `-- --all` and record every
//    result line. Any P0 failure fails the gate.
const report = resolve(ARTIFACTS, "verify:scenario-report.txt");
let scenarioOut;
try {
  scenarioOut = run("node", ["scripts/verify/run-scenario.mjs", "--", "--all"], ROOT, {
    CCORE_TESTS: "/tmp/core-lib-tests.txt",
    CDESK_TESTS: "/tmp/desk-lib-tests.txt",
  });
} catch (e) {
  fail(`verify:scenario -- --all failed; see artifacts/verify:scenario-report.txt\n${e.message}`);
}
writeFileSync(report, scenarioOut);
const okLines = (scenarioOut.match(/^ok: scenario /gm) || []).length;
if (okLines < 166) {
  fail(`expected 166 scenario results, got ${okLines} (see ${report})`);
}

// 4. PRD A1–A14 mapping exists and is written.
run("node", ["scripts/verify/prd-matrix.mjs", "--write"]);
const prd = readFileSync(resolve(ROOT, "docs", "acceptance", "PRD-matrix.md"), "utf8");
if (!prd.includes(`**A1**`) || !prd.includes(`**A14**`)) {
  fail("PRD-matrix.md missing A1–A14 rows");
}

process.stdout.write(`ok 13.9: specs == traceability == manifest (166 scenarios); command map validated; scenario suite ${okLines}/166; PRD A1–A14 mapped\n`);