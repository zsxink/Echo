#!/usr/bin/env node
// Task 7.6: static proof-chain gate for every registered check. A check must
// have an observable failure route and at least one assertion/child-command
// observation. This keeps a new no-op check out of the effective gate set.
//
// Second rule, added because it is the one silent-pass shape the first rule
// cannot see: a check that shells out and then *ignores* the child's exit
// status reads as "observes a child command" while in fact passing whenever the
// child fails. Every spawning check must compare `.status` against something.
// (`injection-suite.mjs` proves the same property dynamically on a sample.)

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const manifest = JSON.parse(readFileSync(resolve(ROOT, "scripts/verify/manifest.json"), "utf8"));
const checks = new Set();
for (const task of manifest.tasks || []) for (const command of task.commands || []) {
  // Every check registered through the manifest is in scope, whether it lives in
  // `checks/` (task checks) or at the verify root (repository-wide gates).
  const match = command.cmd.match(
    /(?:^|\s)node\s+(scripts\/verify\/(?:checks\/)?[A-Za-z0-9_.-]+\.mjs)/,
  );
  if (match) checks.add(match[1]);
}
const errors = [];
for (const path of [...checks].sort()) {
  if (!existsSync(resolve(ROOT, path))) { errors.push(`${path}: script is missing`); continue; }
  const source = readFileSync(resolve(ROOT, path), "utf8");
  const failure = /process\.exit\(1\)|process\.exitCode\s*=\s*1|status\s*!==\s*0|\bfail\s*\(/.test(source);
  const observation = /\bif\s*\(|\.includes\(|\.match\(|\.test\(|spawnSync\(|execFileSync\(|readFileSync\(/.test(source);
  if (!failure) errors.push(`${path}: no observable non-zero failure route`);
  if (!observation) errors.push(`${path}: no observable assertion or child-command evidence`);
  const spawns = /spawnSync\(|execFileSync\(/.test(source);
  if (spawns && !/\.status\s*(?:!==|===|!=|==)/.test(source)) {
    errors.push(`${path}: spawns a child but never compares its exit status (silent pass)`);
  }
}
if (errors.length) {
  process.stderr.write(`FAIL verification validity:\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write(`ok verification validity: ${checks.size} registered checks expose failure and evidence paths\n`);
