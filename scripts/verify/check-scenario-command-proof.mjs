#!/usr/bin/env node
// Governance check — scenario acceptance commands must carry an executable
// failure proof (`engineering-governance`: 作为场景验收命令的检查必须自带可执行的
// 失败证明).
//
// Every scenario whose acceptance command is a node check script
// (`node scripts/verify/checks/<…>.mjs`) relies on that script to fail when the
// condition its THEN clause depends on is violated. This gate inspects each
// such script and classifies how its failure routes are proven:
//
//   proven           — an injection-suite entry (`injection-suite.mjs` /
//                      `injection-object-kinds.mjs`) targets the script with a
//                      repository-mutation mechanism: the suite really violates
//                      the guarded condition and watches the script fail. This
//                      proves the script's self-contained assertions.
//   self-contained   — the script shells out to no subprocess (no
//                      `spawnSync`/`execFileSync`); per the governance rule it
//                      passes the classification (its assertions are still
//                      expected to gain entries over time).
//   pure delegation  — the script spawns children but reads no repository
//                      file: every failure route is a child's nonzero exit,
//                      which the delegation family (`delegation/cargo-exit-status`,
//                      `delegation/pnpm-exit-status`, `delegation/runner-propagates-check-failure`)
//                      proves structurally for all delegating checks.
//   declared         — a *hybrid* (delegates + asserts on repository content)
//                      whose assertions are not yet proven may be explicitly
//                      declared in `scenario-command-proof-declarations.json`
//                      (script → reason). The declaration replaces the old
//                      silent binary exemption ("contains spawnSync ⇒ wholesale
//                      waiver") with a named, reasoned, frozen ratchet: any NEW
//                      undeclared hybrid fails this gate, and removing a
//                      declaration requires adding an injection entry.
//
// Fails (nonzero) when a scenario-backed hybrid check is neither proven nor
// declared, or when a declaration references a missing script.

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const SUITE = resolve(ROOT, "scripts", "verify", "injection-suite.mjs");
const OBJECT_KINDS = resolve(ROOT, "scripts", "verify", "injection-object-kinds.mjs");
const GOVERNANCE = resolve(ROOT, "scripts", "verify", "injection-governance.mjs");
const DECLARATIONS = resolve(ROOT, "scripts", "verify", "scenario-command-proof-declarations.json");

function fail(msg) {
  process.stderr.write(`FAIL scenario-command-proof: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// 1. Scenario-backed check scripts.
// ---------------------------------------------------------------------------
const scriptToScenarios = new Map();
for (const scenario of scenarioCommands()) {
  const match = scenario.command.match(/node (scripts\/verify\/checks\/[A-Za-z0-9_.-]+\.mjs)/);
  if (!match) continue;
  if (!scriptToScenarios.has(match[1])) scriptToScenarios.set(match[1], []);
  scriptToScenarios.get(match[1]).push(scenario.id);
}
if (scriptToScenarios.size === 0) {
  fail("no scenario resolves to a node check script — the map is empty, which is itself suspicious");
}

// ---------------------------------------------------------------------------
// 2. Injection-suite entries: id, targeted script, and mutation mechanism.
//    Parsed from source (the suite executes at import time, so it cannot be
//    imported). Same approach as design.md's script A.
// ---------------------------------------------------------------------------
const MUTATION_HELPERS = [
  "replaceIn(",
  "replaceAllIn(",
  "replaceWithinBlock(",
  "writeTarget(",
  "createProbe(",
  "recordKindFixture(",
];

function parseEntries(source, file) {
  const entries = [];
  const idPattern = /id:\s*"([^"]+)"/g;
  const marks = [...source.matchAll(idPattern)];
  for (let i = 0; i < marks.length; i += 1) {
    const start = marks[i].index;
    const end = i + 1 < marks.length ? marks[i + 1].index : source.length;
    const body = source.slice(start, end);
    const checkMatch = body.match(/check:\s*\["node",\s*\[\s*"([^"]+\.mjs)"/);
    if (!checkMatch) continue;
    const injectMatch = body.match(/inject\(\)\s*\{([\s\S]*?)\n\s*\},/);
    const injectSource = injectMatch ? injectMatch[1] : "";
    entries.push({
      id: marks[i][1],
      file,
      script: checkMatch[1],
      mutatesRepository: MUTATION_HELPERS.some((helper) => injectSource.includes(helper)),
    });
  }
  return entries;
}

const entries = [
  ...parseEntries(readFileSync(SUITE, "utf8"), "injection-suite.mjs"),
  ...parseEntries(readFileSync(OBJECT_KINDS, "utf8"), "injection-object-kinds.mjs"),
  ...parseEntries(readFileSync(GOVERNANCE, "utf8"), "injection-governance.mjs"),
];

const provenByScript = new Map();
for (const entry of entries) {
  if (!entry.mutatesRepository) continue;
  if (!provenByScript.has(entry.script)) provenByScript.set(entry.script, []);
  provenByScript.get(entry.script).push(`${entry.id} (${entry.file})`);
}

// A target script that no longer exists means a stale entry: the proof proves
// nothing.
for (const script of new Set(entries.map((e) => e.script))) {
  if (!existsSync(resolve(ROOT, script))) {
    fail(`injection entry targets a missing script: ${script}`);
  }
}

// ---------------------------------------------------------------------------
// 3. Declarations (the ratchet).
// ---------------------------------------------------------------------------
let declarations = {};
if (existsSync(DECLARATIONS)) {
  declarations = JSON.parse(readFileSync(DECLARATIONS, "utf8")).declared || {};
}

// ---------------------------------------------------------------------------
// 4. Classify every scenario-backed script.
// ---------------------------------------------------------------------------
const unproven = [];
const summary = { proven: [], selfContained: [], pureDelegation: [], declared: [] };
for (const [script, scenarios] of [...scriptToScenarios.entries()].sort()) {
  const abs = resolve(ROOT, script);
  if (!existsSync(abs)) {
    fail(`scenario(s) ${scenarios.join(",")} point to a missing check script: ${script}`);
  }
  const source = readFileSync(abs, "utf8");
  const delegating = /spawnSync\(|execFileSync\(/.test(source);
  const readsRepository = /readFileSync\(/.test(source);

  if (provenByScript.has(script)) {
    summary.proven.push(`${script} [${provenByScript.get(script).join(", ")}]`);
    continue;
  }
  if (!delegating) {
    summary.selfContained.push(script);
    continue;
  }
  if (!readsRepository) {
    // Every failure route derives from a child's exit status; the delegation
    // family covers that structurally. Internal content assertions would show
    // up as readFileSync.
    summary.pureDelegation.push(script);
    continue;
  }
  // Hybrid: delegates AND asserts on repository content. Its internal
  // assertions need a proof or an explicit declaration — never a silent pass.
  const reason = declarations[script];
  if (typeof reason === "string" && reason.trim()) {
    summary.declared.push(`${script} — ${reason.trim()}`);
    continue;
  }
  unproven.push(
    `${script} backs scenario(s) ${scenarios.join(",")}: hybrid check (delegates + repository-content assertions) has no mutation-backed injection entry and no declaration`,
  );
}

if (unproven.length) {
  process.stderr.write(
    `FAIL scenario-command-proof: ${unproven.length} scenario-backed check(s) lack an executable failure proof for their self-contained assertions:\n` +
      `${unproven.map((line) => `- ${line}`).join("\n")}\n` +
      `add a mutation-backed entry to scripts/verify/injection-suite.mjs, or (only as an explicit,\n` +
      `reasoned ratchet) declare the gap in scripts/verify/scenario-command-proof-declarations.json\n`,
  );
  process.exit(1);
}

for (const [label, items] of Object.entries(summary)) {
  process.stdout.write(`  ${label}: ${items.length}\n`);
  for (const item of items) process.stdout.write(`    - ${item}\n`);
}
process.stdout.write(
  "ok scenario-command-proof: every scenario-backed check has a declared failure route\n",
);
