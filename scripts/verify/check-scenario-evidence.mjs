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

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
// The registry is the source of truth; the generated manifest is drift-locked
// to it by gen-scenario-manifests.mjs. Reading the registry here means an
// injection that re-points an evidence target is caught before regeneration.
const scenarios = scenarioCommands();

/** Measured when the evidence registry replaced the churn ratchet. */
const FLOORS = { scenarios: 523, evidence: 541 };
const KINDS = new Set(["rust", "vitest", "node", "attestation"]);
const DELEGATED_GATES = new Set(["governance"]);

const errors = [];
const fail = (m) => errors.push(m);

// A delegation only means something if the named gate actually runs in CI.
const ciWorkflow = readFileSync(resolve(ROOT, ".github", "workflows", "ci.yml"), "utf8");
const CI_GATES = new Map(
  [...DELEGATED_GATES].map((gate) => [gate, new RegExp(`\\bpnpm\\s+verify:${gate}\\b`).test(ciWorkflow)]),
);
for (const [gate, present] of CI_GATES) {
  if (!present) fail(`delegated gate '${gate}' has no job running 'pnpm verify:${gate}' in .github/workflows/ci.yml`);
}
let delegatedCount = 0;

const testLists = new Map();
function cargoTestList(packageName) {
  if (testLists.has(packageName)) return testLists.get(packageName);
  // `--all-targets` keeps this listing in the same build shape as the workspace
  // test job; without it Cargo also builds the doc-test harness, which no
  // scenario evidence can come from.
  const result = spawnSync("cargo", ["test", "-p", packageName, "--all-targets", "--all-features", "--", "--list"], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const tests = result.status === 0
    ? `${result.stdout || ""}\n${result.stderr || ""}`
      .split("\n")
      .filter((line) => /: test$/.test(line))
      .map((line) => line.replace(/: test$/, "").trim())
    : [];
  if (result.status !== 0) {
    fail(`could not list ${packageName} tests (cargo exited ${result.status})`);
  }
  testLists.set(packageName, tests);
  return tests;
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
