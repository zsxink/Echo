#!/usr/bin/env node
// Unified scenario verification runner.
//
// Usage:
//   pnpm verify:scenario -- <scenario-id...>   run the listed scenarios
//   pnpm verify:scenario -- --all              run every registered scenario
//   pnpm verify:scenario -- --list             print scenario ids from the manifest
//
// Scenarios are resolved/validated against scripts/verify/manifest.json's
// `scenarios` section and executed against the test manifest. The scenario set
// is reconciled with the specs' Scenario IDs at release time (task 13.9);
// here we fail fast on unknown, missing-command or failing scenarios.
//
// The runner never modifies lockfiles.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const MANIFEST = resolve(ROOT, "scripts", "verify", "manifest.json");

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exitCode = 1;
}

const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const scenarios = manifest.scenarios || [];

function runScenario(id) {
  const sc = scenarios.find((s) => s.id === id);
  if (!sc) {
    fail(`unknown scenario id: ${id}`);
    return false;
  }
  if (!sc.command || !sc.command.trim()) {
    fail(`scenario ${id}: no verification command registered`);
    return false;
  }
  const result = spawnSync(sc.command, {
    cwd: ROOT,
    shell: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    fail(`scenario ${id}: command exited ${result.status}: ${sc.command}`);
    return false;
  }
  process.stdout.write(`ok: scenario ${id} (${sc.title})\n`);
  return true;
}

const argv = process.argv.slice(2);

if (argv.includes("--list")) {
  for (const s of scenarios) process.stdout.write(`${s.id}\n`);
  process.exit(0);
}

let requested;
if (argv.includes("--all")) {
  requested = scenarios.map((s) => s.id);
} else {
  const ids = argv.flatMap((a) => (a === "--all" ? [] : a.split(/\s+/)));
  requested = ids.filter(Boolean);
}
if (!requested.length) {
  fail("no scenario ids given (use -- <id...> or -- --all)");
  process.exit(1);
}

let ok = true;
for (const id of requested) {
  if (!runScenario(id)) ok = false;
}
process.exit(ok ? 0 : 1);
