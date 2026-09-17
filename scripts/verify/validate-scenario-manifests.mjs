#!/usr/bin/env node
// Task 7.5: parse every executable YAML scenario manifest and assert that its
// meaningful fields agree with the executable scenario registry.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { allScenarioIds } from "./spec-scenarios.mjs";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const SCENARIOS = resolve(ROOT, "tests", "scenarios");
const manifest = JSON.parse(readFileSync(resolve(ROOT, "scripts/verify/manifest.json"), "utf8"));
const byId = new Map((manifest.scenarios || []).map((entry) => [entry.id, entry]));
const derived = new Map(allScenarioIds().map((entry) => [entry.id, entry]));
const commands = new Map(scenarioCommands().map((entry) => [entry.id, entry]));
const errors = [];

function scalar(text, key, file) {
  const match = text.match(new RegExp(`^${key}:\\s*(.*)$`, "m"));
  if (!match || !match[1].trim()) {
    errors.push(`${file}: missing non-empty ${key}`);
    return "";
  }
  return match[1].trim().replace(/^"|"$/g, "");
}

for (const name of readdirSync(SCENARIOS).filter((entry) => entry.endsWith(".yaml"))) {
  const file = `tests/scenarios/${name}`;
  const text = readFileSync(resolve(ROOT, file), "utf8");
  const id = scalar(text, "scenario_id", file);
  const entry = byId.get(id) || commands.get(id);
  const spec = derived.get(id);
  const requirement = scalar(text, "requirement", file);
  const scenario = scalar(text, "scenario", file);
  const layer = scalar(text, "layer", file);
  const command = scalar(text, "command", file);
  scalar(text, "expected_result", file);
  if (!entry || !spec) {
    // Keep parsing legacy checked-in assets even when a concurrent spec update
    // has removed their registry row; the reconciliation gate owns registry
    // membership, while this gate owns field validity for every YAML file.
    if (layer !== "automated") errors.push(`${file}: layer must be automated`);
    if (!command) errors.push(`${file}: command must be non-empty`);
    console.warn(`warning scenario manifest: ${file} is not in the current registry`);
    continue;
  }
  if (byId.has(id) && entry.manifest !== file) errors.push(`${file}: manifest points to ${entry.manifest}`);
  if (requirement !== spec.requirement) errors.push(`${file}: requirement drift`);
  if (scenario !== entry.title) errors.push(`${file}: scenario title drift`);
  if (layer !== "automated") errors.push(`${file}: layer must be automated`);
  if (command !== entry.command) errors.push(`${file}: command drift`);
  if (!/^automated_filter:\s+the command above$/m.test(text)) errors.push(`${file}: automated_filter does not bind the executable command`);
  if (!/^fixtures:\s+\[/m.test(text)) errors.push(`${file}: fixtures field is missing`);
}

for (const entry of manifest.scenarios || []) {
  if (entry.manifest?.startsWith("tests/scenarios/") && !existsSync(resolve(ROOT, entry.manifest))) {
    errors.push(`${entry.id}: referenced YAML is missing`);
  }
}

if (errors.length) {
  process.stderr.write(`FAIL scenario manifest validation:\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write(`ok scenario manifests: parsed and checked ${readdirSync(SCENARIOS).filter((entry) => entry.endsWith(".yaml")).length} YAML files\n`);
