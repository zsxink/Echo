#!/usr/bin/env node
// Task 5.1: source-file and public-trait scale gate.
//
// The ratchet has two files, one purpose each:
//   - scripts/verify/scale-baseline.json — the frozen maximum set. It records
//     which oversized files / wide traits existed when the limit was adopted.
//     Editing it to add an entry is exactly what this gate exists to prevent.
//   - scripts/verify/scale-allowlist.json — the *active* exception list. It may
//     shrink as files are split, but every entry must still be in the baseline.
// Keeping the two separate means the exception list and the frozen ceiling are
// not two copies of the same data: the comparison below is the single source
// of truth for "did the allowlist grow?".

import { readFileSync, readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const ROOT = resolve(process.env.ECHO_SCALE_ROOT || DEFAULT_ROOT);
const ALLOWLIST_PATH = resolve(DEFAULT_ROOT, "scripts", "verify", "scale-allowlist.json");
const BASELINE_PATH = resolve(DEFAULT_ROOT, "scripts", "verify", "scale-baseline.json");

const SOURCE_EXTENSIONS = new Set([".css", ".js", ".jsx", ".mjs", ".rs", ".ts", ".tsx"]);
const IGNORED_PARTS = new Set([".git", "node_modules", "target", "dist", "build"]);
const errors = [];

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory() && IGNORED_PARTS.has(entry.name)) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(path));
    else out.push(path);
  }
  return out;
}

function sorted(value) { return [...value].sort(); }

const allowlist = JSON.parse(readFileSync(ALLOWLIST_PATH, "utf8"));
const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
const listsAreWellFormed =
  Array.isArray(allowlist.files) &&
  Array.isArray(allowlist.traits) &&
  Array.isArray(baseline.files) &&
  Array.isArray(baseline.traits);
if (!listsAreWellFormed) {
  errors.push("scale allowlist/baseline must each carry `files` and `traits` arrays");
} else {
  // The allowlist may only shrink relative to the frozen baseline: every entry
  // it keeps must have been recorded in the baseline. Running the comparison
  // this direction (subset, not equality) is what makes "shrink-only" mean
  // something — an allowlist entry with no baseline counterpart is an expansion.
  const outsideBaseline = [
    ...sorted(allowlist.files).filter((v) => !baseline.files.includes(v)),
    ...sorted(allowlist.traits).filter((v) => !baseline.traits.includes(v)),
  ];
  if (outsideBaseline.length) {
    errors.push(
      "scale allowlist was expanded or changed; only removing baseline entries is allowed" +
        ` (not in scale-baseline.json: ${outsideBaseline.join(", ")})`,
    );
  }
}
const allowedFiles = new Set(allowlist.files || []);
const allowedTraits = new Set(allowlist.traits || []);

for (const path of walk(ROOT)) {
  // Normalize to forward slashes so the allowlist keys match on every platform
  // (on Windows `relative()` yields backslashes and no allowlisted file or
  // trait ever matched, red-ing machines CI keeps green).
  const rel = relative(ROOT, path).replaceAll("\\", "/");
  const ext = rel.slice(rel.lastIndexOf("."));
  if (!SOURCE_EXTENSIONS.has(ext) || /(^|\/)(generated|gen)(\/|\.)|\.generated\./.test(rel)) continue;
  const source = readFileSync(path, "utf8");
  const lines = source.split("\n").length;
  if (lines > 1000 && !allowedFiles.has(rel)) errors.push(`${rel}: ${lines} lines exceeds 1000`);

  if (ext !== ".rs") continue;
  for (const match of source.matchAll(/pub\s+trait\s+([A-Za-z_][A-Za-z0-9_]*)[^{]*\{/g)) {
    const start = match.index + match[0].length;
    let depth = 1;
    let end = start;
    for (; end < source.length && depth; end += 1) {
      if (source[end] === "{") depth += 1;
      if (source[end] === "}") depth -= 1;
    }
    const body = source.slice(start, end - 1);
    const methods = [...body.matchAll(/\bfn\s+[A-Za-z_][A-Za-z0-9_]*\s*\(/g)].length;
    const key = `${rel}::${match[1]}`;
    if (methods > 6 && !allowedTraits.has(key)) errors.push(`${key}: ${methods} methods exceeds 6`);
  }
}

if (errors.length) {
  process.stderr.write(`FAIL scale gate:\n${errors.map((e) => `- ${e}`).join("\n")}\n`);
  process.exit(1);
}
process.stdout.write("ok scale gate: source-file and public-trait limits hold; allowlist did not grow\n");
