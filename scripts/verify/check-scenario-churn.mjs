#!/usr/bin/env node
// Task 7.7: scenario-command churn ratchet.
//
// Scenarios are executable acceptance lines, but today many of them share one
// broad command (originally 209 scenarios over 115 distinct commands ≈ 1.82×,
// with one command cited 16 times). A single broken command therefore blanks a
// whole cluster of scenarios at once.
//
// A full "aggregate by module" rewrite touches every acceptance line at once and
// risks silently re-pointing scenarios at the wrong tests. This gate takes the
// safer half first: it freezes what is measurable so the ratio can only go
// **down**. Raising a ceiling means editing this file, which is reviewable.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const manifest = JSON.parse(readFileSync(resolve(ROOT, "scripts", "verify", "manifest.json"), "utf8"));

/** Frozen ceilings measured at Task 7.7 time — only downwards may move. */
const CEILINGS = {
  maxDuplication: 1.8,
  maxScenariosPerCommand: 16,
  /** Commands that may not be referenced more than once at all. */
  forbiddenSharedRail: "none",
};

const scenarios = manifest.scenarios || [];
const byCommand = new Map();
for (const scenario of scenarios) {
  const key = scenario.command || "";
  if (!key) continue;
  byCommand.set(key, (byCommand.get(key) || 0) + 1);
}

const total = [...byCommand.values()].reduce((sum, n) => sum + n, 0);
const distinct = byCommand.size;
const duplication = distinct === 0 ? 0 : total / distinct;
const worst = [...byCommand.entries()].sort((a, b) => b[1] - a[1])[0] || ["", 0];

const errors = [];
if (duplication > CEILINGS.maxDuplication) {
  errors.push(
    `duplication ${duplication.toFixed(2)}x exceeds frozen ceiling ${CEILINGS.maxDuplication}x`,
  );
}
if (worst[1] > CEILINGS.maxScenariosPerCommand) {
  errors.push(
    `${worst[1]} scenarios share one command (ceiling ${CEILINGS.maxScenariosPerCommand}): ${worst[0]}`,
  );
}
if (byCommand.get(CEILINGS.forbiddenSharedRail)) {
  errors.push(`command must not be shared between scenarios: ${CEILINGS.forbiddenSharedRail}`);
}

const summary = `${total} scenarios over ${distinct} commands (${duplication.toFixed(2)}x); worst cluster ${worst[1]}`;
if (errors.length) {
  process.stderr.write(`FAIL scenario churn (${summary}):\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write(`ok scenario churn: ${summary} within frozen ceilings\n`);
