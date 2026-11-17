#!/usr/bin/env node
// Scenario-evidence gate (replaces the scenario-churn ratchet, task 14.7).
//
// Aggregating commands by domain means "the command succeeded" no longer
// implies "this scenario's test ran": one `cargo test -p echo-core
// --all-features` now backs dozens of acceptance lines. The evidence
// declarations in `scenario-commands.mjs` (generated into
// `manifest.json.scenarios[]`) restore that link, so this gate guards the link
// itself:
//
//   - every scenario has a non-empty `evidence` array whose entries have a
//     legal `kind` and a non-empty `target`;
//   - `rust` targets match a test in the `cargo test -- --list` set of a
//     package the scenario's command runs;
//   - `vitest` files exist, and a `name` appears in the file;
//   - `node` targets exist inside `scripts/verify/`;
//   - `attestation` targets are the scenario id itself;
//   - `delegatedTo`, when present, names an allowed gate that a real CI job
//     runs (`pnpm verify:governance`), so a scenario cannot claim coverage by
//     a gate that never executes.
//
// It also freezes the measured floor: 523 scenarios and 541 evidence entries.
// Both numbers may only grow. Removing an evidence element therefore requires
// editing this floor in the same change and explaining why that scenario's
// acceptance no longer needs the target — that edit is the reviewable record.

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";
import { workspaceTestLists } from "./cargo-test-lists.mjs";
import { matrixEntries, parseWorkflow } from "./workflow-matrix.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
// The registry is the source of truth; the generated manifest is drift-locked
// to it by gen-scenario-manifests.mjs. Reading the registry here means an
// injection that re-points an evidence target is caught before regeneration.
const scenarios = scenarioCommands();

/** Measured when the evidence registry replaced the churn ratchet. */
const FLOORS = { scenarios: 523, evidence: 541 };
const KINDS = new Set(["rust", "vitest", "node", "attestation"]);
const DELEGATED_GATES = new Set(["governance", "perf", "workspace-test", "frontend-test"]);

const errors = [];
const fail = (m) => errors.push(m);

// A delegation only means something if the named gate actually runs in CI.
const ciWorkflow = readFileSync(resolve(ROOT, ".github", "workflows", "ci.yml"), "utf8");
// Each gate names the exact command ci.yml must execute. `workspace-test` and
// `frontend-test` are the full suites the Linux `rust` / `frontend` jobs run;
// the other two are governance commands. The workspace suite is written as
// `cargo test ${{ matrix.packages }} ...` with `--workspace` in the matrix, so
// it is verified structurally rather than by a literal-string regex; a comment
// mentioning a command cannot satisfy either check.
let CI_GATES = new Map();
try {
  const workflow = parseWorkflow(ciWorkflow, "ci.yml");
  const linux = matrixEntries(workflow, "rust", "ci.yml").find((entry) => entry.platform === "Linux");
  const rustTest = (workflow.jobs?.rust?.steps || []).find((s) => s.name === "Test");
  const frontendTest = (workflow.jobs?.frontend?.steps || []).find((s) => s.name === "Test");
  CI_GATES = new Map([
    ["governance", /\bpnpm\s+verify:governance\b/.test(ciWorkflow)],
    ["perf", /\bpnpm\s+verify:perf\b/.test(ciWorkflow)],
    [
      "workspace-test",
      linux?.packages === "--workspace" &&
        String(rustTest?.run || "").trim() === "cargo test ${{ matrix.packages }} --all-targets --all-features",
    ],
    ["frontend-test", String(frontendTest?.run || "").includes("pnpm --filter @echo/desktop test -- --run")],
  ]);
} catch (error) {
  fail(`could not parse .github/workflows/ci.yml: ${error.message}`);
}
for (const [gate, present] of CI_GATES) {
  if (!present) fail(`delegated gate '${gate}' has no matching command in .github/workflows/ci.yml`);
}
let delegatedCount = 0;

const testLists = new Map();
// Same reason as validate-scenario-commands.mjs: one workspace-shaped build
// serves every package's inventory, so no per-package `--list` re-unifies
// features (echo-desktop enables echo-core/testkit) and recompiles minutes'
// worth of work on each run.
const inventories = workspaceTestLists({ cwd: ROOT });
if (inventories.error) fail(`could not list workspace tests: ${inventories.error}`);
function cargoTestList(packageName) {
  if (!testLists.has(packageName)) testLists.set(packageName, inventories.tests.get(packageName) || []);
  return testLists.get(packageName);
}

let evidenceCount = 0;
for (const scenario of scenarios) {
  const id = scenario.id;
  const evidence = scenario.evidence;
  if (!Array.isArray(evidence) || evidence.length === 0) {
    fail(`${id}: no evidence declared`);
    continue;
  }
  evidenceCount += evidence.length;
  if (scenario.delegatedTo !== undefined) {
    delegatedCount += 1;
    if (!DELEGATED_GATES.has(scenario.delegatedTo)) {
      fail(`${id}: delegatedTo '${scenario.delegatedTo}' is not an allowed gate (${[...DELEGATED_GATES].join(", ")})`);
    }
  }
  const packages = [...new Set(
    [...(scenario.command || "").matchAll(/-p (echo-core|echo-desktop|echo-app)/g)].map((m) => m[1]),
  )];
  for (const item of evidence) {
    if (!item || typeof item !== "object" || typeof item.target !== "string" || !item.target) {
      fail(`${id}: evidence entry without a target`);
      continue;
    }
    if (!KINDS.has(item.kind)) {
      fail(`${id}: unknown evidence kind '${item.kind}'`);
      continue;
    }
    if (item.kind === "rust") {
      if (packages.length === 0) {
        fail(`${id}: rust evidence '${item.target}' but the command runs no cargo package`);
      } else if (!packages.flatMap((pkg) => cargoTestList(pkg)).some((t) => t.includes(item.target))) {
        fail(`${id}: rust evidence '${item.target}' matches 0 tests in ${packages.join("/")}`);
      }
    } else if (item.kind === "vitest") {
      const p = resolve(ROOT, "apps", "desktop", item.target);
      if (!existsSync(p)) fail(`${id}: vitest evidence file missing ${item.target}`);
      else if (item.name && !readFileSync(p, "utf8").includes(item.name)) {
        fail(`${id}: vitest evidence name '${item.name}' is not in ${item.target}`);
      }
    } else if (item.kind === "node") {
      if (item.target.includes("..") || !item.target.startsWith("scripts/verify/")) {
        fail(`${id}: node evidence escapes scripts/verify: ${item.target}`);
      } else if (!existsSync(resolve(ROOT, item.target))) {
        fail(`${id}: node evidence missing ${item.target}`);
      }
    } else if (item.kind === "attestation") {
      if (item.target !== id) fail(`${id}: attestation target must be the scenario id, got '${item.target}'`);
    }
  }
}

if (scenarios.length < FLOORS.scenarios) {
  fail(`${scenarios.length} scenarios is below the frozen floor ${FLOORS.scenarios}`);
}
if (evidenceCount < FLOORS.evidence) {
  fail(`${evidenceCount} evidence entries is below the frozen floor ${FLOORS.evidence}`);
}

const summary = `${scenarios.length} scenarios, ${evidenceCount} evidence entries, ${delegatedCount} delegated (floors ${FLOORS.scenarios}/${FLOORS.evidence})`;
if (errors.length) {
  process.stderr.write(`FAIL scenario evidence (${summary}):\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write(`ok scenario evidence: ${summary}\n`);
