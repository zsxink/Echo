#!/usr/bin/env node
// Generate (and verify) the executable scenario set from the command registry
// (task 13.9).
//
//   node scripts/verify/gen-scenario-manifests.mjs           # drift gate
//   node scripts/verify/gen-scenario-manifests.mjs --write   # regenerate
//
// `manifest.json.scenarios[]` is the only generated artifact: one
// `{id, title, command}` entry per spec-derived scenario. `run-scenario.mjs`
// executes the command recorded there, so the default mode below is the gate
// that keeps the executed set equal to `scenario-commands.mjs`: any missing,
// extra, retitled or re-commanded row exits nonzero.
//
// `--write` is the only writer. The former per-scenario mirror trees are gone;
// their contract lives in `scenario-commands.mjs`,
// `docs/native-attestation-playbook.md` and the retained template file.

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { allScenarioIds } from "./spec-scenarios.mjs";
import { scenarioCommands } from "./scenario-commands.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const MANIFEST_PATH = resolve(ROOT, "scripts", "verify", "manifest.json");

const ids = allScenarioIds();
const commands = scenarioCommands();

// The two sources must agree before either can be compared with the manifest.
const byId = new Map();
let errs = 0;
for (const d of ids) {
  if (byId.has(d.id)) { console.error(`duplicate spec scenario: ${d.id}`); errs++; }
  byId.set(d.id, d);
}
const commandById = new Map();
for (const c of commands) {
  if (commandById.has(c.id)) { console.error(`duplicate command: ${c.id}`); errs++; }
  commandById.set(c.id, c);
}
for (const d of ids) if (!commandById.has(d.id)) { console.error(`missing command for ${d.id}`); errs++; }
for (const c of commands) if (!byId.has(c.id)) { console.error(`orphan command: ${c.id}`); errs++; }
if (errs) process.exit(1);

const entries = ids.map((d) => ({
  id: d.id,
  title: d.scenario,
  command: commandById.get(d.id).command,
}));

if (process.argv.includes("--write")) {
  // Preserve task entries exactly; only scenarios are regenerated.
  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  manifest.scenarios = entries;
  writeFileSync(MANIFEST_PATH, `${JSON.stringify(manifest, null, 2)}\n`);
  process.stdout.write(`wrote manifest.scenarios (${entries.length})\n`);
  process.exit(0);
}

// Drift gate: compare the committed scenarios with the derived entries by ID.
const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
const committed = manifest.scenarios ?? [];
const committedById = new Map();
let drift = 0;
const report = (m) => { console.error(`manifest drift: ${m}`); drift++; };

for (const s of committed) {
  if (committedById.has(s.id)) report(`${s.id}: duplicate entry in manifest.json`);
  committedById.set(s.id, s);
}
for (const e of entries) {
  const c = committedById.get(e.id);
  if (!c) { report(`${e.id}: missing from manifest.json scenarios`); continue; }
  if (c.title !== e.title) report(`${e.id}: title drift (manifest "${c.title}" != spec "${e.title}")`);
  if (c.command !== e.command) report(`${e.id}: command drift (manifest "${c.command}" != registry "${e.command}")`);
}
for (const s of committed) {
  if (!byId.has(s.id)) report(`${s.id}: extra entry has no spec scenario`);
}

if (drift) {
  console.error(`FAIL scenario registry drift: ${drift} problem(s)`);
  process.exit(1);
}
process.stdout.write(`ok scenario registry: ${entries.length} scenarios, manifest drift: 0\n`);
