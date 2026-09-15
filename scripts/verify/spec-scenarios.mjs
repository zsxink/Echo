#!/usr/bin/env node
// Authoritative scenario-ID assigner for the 0.1.0 specs.
//
// The spec .md files under
//   openspec/changes/release-0-1-0-desktop-player/specs/<area>/spec.md
// carry `### Requirement: <title>` and `#### Scenario: <title>` headings with
// NO inline IDs. The stable IDs live in `traceability.md` (the per-area
// prefix map is fixed there: DAS/DP/IL/LE/LL/PM/SFI).
//
// This script derives `<AREA>-R<NN>-S<NN>` deterministically from heading
// order in each spec file and emits the ordered list. It is the authoritative
// source the reconciliation validator (reconcile-scenarios.mjs) checks
// `traceability.md` and `manifest.json` against, so any edit to the spec
// files that reorders/renames requirements or scenarios is caught as a
// three-way-set mismatch instead of silently renumbering.
//
// Guarantee: for the current specs it MUST reproduce traceability's 160 IDs
// byte-for-byte (per-area counts: DAS 27, DP 24, IL 27, LE 23, LL 23, PM 14,
// SFI 22).

import { readdirSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const SPECS_DIR = resolve(
  ROOT,
  "openspec",
  "changes",
  "release-0-1-0-desktop-player",
  "specs",
);
const CHANGE_DIR = resolve(SPECS_DIR, "..");

// The per-area prefix is fixed by traceability.md; an area maps 1:1 to a
// directory and a prefix. Order here is the stable publication order.
const AREA_PREFIX = {
  "desktop-app-shell": "DAS",
  "desktop-playback": "DP",
  "immersive-lyrics": "IL",
  "library-experience": "LE",
  "local-library": "LL",
  "playlist-management": "PM",
  "safe-file-ingestion": "SFI",
  "sync-foundation": "SYN",
};

/**
 * Parse one spec file into ordered requirements with their ordered scenarios.
 *
 * @param {string} area  the spec directory name
 * @returns {{area: string, prefix: string, requirements: Array<{
 *   title: string,
 *   number: number,
 *   scenarios: Array<{ title: string, number: number, id: string }>
 * }>}}
 */
export function parseSpec(area) {
  const path = resolve(SPECS_DIR, area, "spec.md");
  const text = readFileSync(path, "utf8");
  const prefix = AREA_PREFIX[area];
  if (!prefix) throw new Error(`no prefix for spec area '${area}'`);

  const requirements = [];
  let current = null;
  const lines = text.split("\n");
  for (const line of lines) {
    let m = line.match(/^### Requirement: (.+)$/);
    if (m) {
      const number = requirements.length + 1;
      current = { title: m[1].trim(), number, scenarios: [] };
      requirements.push(current);
      continue;
    }
    m = line.match(/^#### Scenario: (.+)$/);
    if (m && current) {
      const number = current.scenarios.length + 1;
      current.scenarios.push({
        title: m[1].trim(),
        number,
        id: `${prefix}-R${String(current.number).padStart(2, "0")}-S${String(number).padStart(2, "0")}`,
      });
    }
  }
  return { area, prefix, requirements };
}

/**
 * Parse one traceability table region into the set of scenario IDs and the
 * title map, so the assigner can be verified against the existing mapping.
 *
 * @param {string} areaName  the `## <area>` heading, e.g. "desktop-app-shell"
 * @returns {Set<string>}
 */
export function traceabilityIdsFor(areaName) {
  const text = readFileSync(resolve(CHANGE_DIR, "traceability.md"), "utf8");
  const lines = text.split("\n");
  const inSection = new RegExp(`^## ${areaName}$`);
  let sectionStart = lines.findIndex((l) => inSection.test(l.trim()));
  if (sectionStart === -1) return new Set();
  let sectionEnd = lines.length;
  for (let i = sectionStart + 1; i < lines.length; i++) {
    if (/^## /.test(lines[i])) {
      sectionEnd = i;
      break;
    }
  }
  const ids = new Set();
  for (const line of lines.slice(sectionStart, sectionEnd)) {
    const m = line.match(/\b([A-Z]{2,4}-R\d{2}-S\d{2})\b/);
    if (m) ids.add(m[1]);
  }
  return ids;
}

function listAreas() {
  return readdirSync(SPECS_DIR, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => d.name)
    .sort();
}

/**
 * The full, ordered scenario list for all specs.
 * @returns {Array<{ id, area, prefix, requirement, requirementNumber,
 *   scenario, scenarioNumber }>}
 */
export function allScenarioIds() {
  const out = [];
  for (const area of listAreas()) {
    const parsed = parseSpec(area);
    for (const req of parsed.requirements) {
      for (const sc of req.scenarios) {
        out.push({
          id: sc.id,
          area,
          prefix: parsed.prefix,
          requirement: req.title,
          requirementNumber: req.number,
          scenario: sc.title,
          scenarioNumber: sc.number,
        });
      }
    }
  }
  return out;
}

// Standalone: print the ordered list as JSON (used by the generator and the
// reconciliation validator). `--check` verifies against traceability.
if (import.meta.url === `file://${process.argv[1]}`) {
  const ids = allScenarioIds();
  if (process.argv.includes("--check")) {
    const trace = new Set();
    for (const area of listAreas()) {
      for (const id of traceabilityIdsFor(area)) trace.add(id);
    }
    const derived = new Set(ids.map((s) => s.id));
    const missing = [...derived].filter((id) => !trace.has(id));
    const extra = [...trace].filter((id) => !derived.has(id));
    if (missing.length || extra.length) {
      process.stderr.write(
        `spec Scenario IDs diverge from traceability.md:\n  missing from trace: ${missing.join(", ")}\n  extra in trace: ${extra.join(", ")}\n`,
      );
      process.exit(1);
    }
    if (trace.size !== ids.length) {
      process.stderr.write(
        `duplicate IDs: spec-derived ${ids.length} unique ${new Set(ids.map((s) => s.id)).size}\n`,
      );
      process.exit(1);
    }
    process.stdout.write(`ok: ${ids.length} spec-derived scenario IDs match traceability.md\n`);
    process.exit(0);
  }
  process.stdout.write(`${JSON.stringify(ids, null, 2)}\n`);
}