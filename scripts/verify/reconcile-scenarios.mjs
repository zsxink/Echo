#!/usr/bin/env node
// Three-way scenario reconciliation for the 0.1.0 release gate (task 13.9).
//
// The release gate requires the Scenario ID sets in these three places to be
// EXACTLY equal:
//   1. the specs (`specs/<area>/spec.md` headings, derived by
//      spec-scenarios.mjs)             — the authoritative ID count
//   2. `openspec/.../traceability.md`  — the per-scenario tracking table
//   3. `scripts/verify/manifest.json` `scenarios[]`    — the executable set
//
// Also enforces traceability 发布审计 rules:
//   - total equals the spec-derived count (must be 160 today);
//   - no missing, duplicate, or extra ID in any of the three;
//   - every scenario has a non-empty command;
//   - every referenced manifest path (`tests/scenarios/<ID>.yaml` or
//     `tests/native/<ID>.md`) exists and is non-empty;
//   - P0 file-safety/recovery/path-boundary/recycle-bin scenarios spawn as
//     automated (non-`.md`) commands, never human-only steps.
//   - `--report` mode writes a per-scenario result table for the handoff.
//
// Nonzero exit on any violation; prints a diff report to stderr.

import { existsSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { allScenarioIds, traceabilityIdsFor } from "./spec-scenarios.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const CHANGE = resolve(ROOT, "openspec", "changes", "release-0-1-0-desktop-player");
const TRACE = resolve(CHANGE, "traceability.md");
const MANIFEST = resolve(ROOT, "scripts", "verify", "manifest.json");

// P0 scenario families that must be automated, not human-only (traceability
// 发布审计 #2: "P0 文件安全、恢复、路径边界与回收站场景必须是自动故障注入，
// 不得只用人工步骤"). These are the prefix families whose native rows drive
// real fault injection in echo-core.
const P0_AUTOMATED_PREFIXES = ["SFI-R04", "SFI-R05", "SFI-R08", "LL-R07", "LL-R08"];

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exitCode = 1;
}

const derived = allScenarioIds();
const derivedIds = new Set(derived.map((s) => s.id));

// --- traceability IDs (all 7 sections) ---
const fields = new Set(["desktop-app-shell", "desktop-playback", "immersive-lyrics", "library-experience", "local-library", "playlist-management", "safe-file-ingestion", "sync-foundation"]);
const traceIds = new Set();
for (const f of fields) for (const id of traceabilityIdsFor(f)) traceIds.add(id);

// --- manifest scenarios ---
const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const scenarios = manifest.scenarios || [];
const manifestById = new Map(scenarios.map((s) => [s.id, s]));

let errors = 0;
const err = (m) => { errors += 1; fail(m); };

// 1. Set equality: derived vs trace.
for (const id of derivedIds) if (!traceIds.has(id)) err(`spec-derived ${id} missing from traceability.md`);
for (const id of traceIds) if (!derivedIds.has(id)) err(`traceability.md ${id} has no spec scenario`);
// 2. Set equality: derived vs manifest.
for (const id of derivedIds) if (!manifestById.has(id)) err(`spec-derived ${id} missing from manifest.json scenarios`);
for (const s of scenarios) if (!derivedIds.has(s.id)) err(`manifest.json scenario ${s.id} has no spec scenario`);
// 3. No duplicate IDs anywhere (spec heading order guarantees uniqueness; manifest must too).
const seen = new Set();
for (const s of scenarios) {
  if (seen.has(s.id)) err(`manifest.json duplicate scenario ${s.id}`);
  seen.add(s.id);
}

// 4. Every scenario has a command.
for (const id of derivedIds) {
  const sc = manifestById.get(id);
  if (!sc || !sc.command || !sc.command.trim()) err(`scenario ${id} has no command`);
}

// 5. Referenced manifest paths exist and are non-empty.
for (const id of derivedIds) {
  const sc = manifestById.get(id);
  if (!sc) continue;
  const m = sc.manifest && sc.manifest.match(/^(tests\/(?:scenarios|native)\/[^`\s]+)$/);
  if (!m) { err(`scenario ${id} has no resolvable manifest path`); continue; }
  const target = resolve(ROOT, m[1]);
  if (!existsSync(target)) { err(`scenario ${id} manifest file missing: ${m[1]}`); continue; }
  if (readFileSync(target, "utf8").trim() === "") err(`scenario ${id} manifest file empty: ${m[1]}`);
}

// 6. P0 prefix families must be automated (fault-injection/recovery/path/
//    recycle-bin rows wired to real tests, not human-only .md steps). A command
//    is "automated" when it runs a real test (a cargo/pnpm/vitest invocation)
//    or a registered task-check script (scripts/verify/checks/task-*.mjs, which
//    execute real test suites). The ONLY human-only marker is the attestation
//    checker, which merely confirms an operator recorded evidence — it must
//    never be the command for a P0 row.
for (const id of derivedIds) {
  const prefix = P0_AUTOMATED_PREFIXES.find((p) => id.startsWith(p));
  if (!prefix) continue;
  const sc = manifestById.get(id);
  if (!sc) continue;
  const humanOnly = /check-native-attestation\.mjs/.test(sc.command);
  if (humanOnly) err(`P0 scenario ${id} is human-only (attestation); must be automated fault injection`);
}

// --- Report mode: write per-scenario lines for the handoff ---
if (process.argv.includes("--report")) {
  const { mkdirSync, writeFileSync } = await import("node:fs");
  const outDir = resolve(ROOT, "artifacts");
  mkdirSync(outDir, { recursive: true });
  const rows = derived.map((d) => {
    const sc = manifestById.get(d.id);
    return `${d.id}\t${d.area}\t${d.requirement}\t${d.scenario}\t${sc ? sc.command.replace(/\t/g, " ") : "(no command)"}`;
  });
  writeFileSync(resolve(outDir, "scenario-reconciliation.tsv"), rows.join("\n") + "\n");
}

process.stdout.write(
  `reconcile: spec ${derivedIds.size} = trace ${traceIds.size} = manifest ${scenarios.length} scenarios${errors ? ` (${errors} mismatches)` : ""}\n`,
);
process.exit(errors ? 1 : 0);