#!/usr/bin/env node
// Task 5.1: source-file and public-trait scale gate.
// The checked-in allowlist is a baseline snapshot. It is intentionally
// compared with the immutable baseline below so adding an exception cannot
// make the gate green; only shrinking the list is permitted.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const ROOT = resolve(process.env.ECHO_SCALE_ROOT || DEFAULT_ROOT);
const ALLOWLIST_PATH = resolve(DEFAULT_ROOT, "scripts", "verify", "scale-allowlist.json");
const BASELINE = {
  files: [
    "crates/echo-core/src/application/recover.rs",
    "crates/echo-core/src/application/scan.rs",
    "crates/echo-core/src/application/testing/memory_database.rs",
    "crates/echo-core/src/domain/entities.rs",
    "crates/echo-core/src/domain/library.rs",
    "crates/echo-core/src/infrastructure/filesystem/adapter.rs",
    "crates/echo-core/src/infrastructure/sqlite/mod.rs",
    "crates/echo-desktop/src/player/actor.rs",
  ],
  traits: [
    "crates/echo-core/src/application/ports/repository.rs::OperationJournalRepository",
    "crates/echo-core/src/application/ports/repository.rs::PlaylistRepository",
    "crates/echo-core/src/application/ports/repository.rs::SongRepository",
    "crates/echo-core/src/application/ports/system.rs::ControlPlanePort",
  ],
};

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
function same(a, b) { return JSON.stringify(sorted(a)) === JSON.stringify(sorted(b)); }

const allowlist = JSON.parse(readFileSync(ALLOWLIST_PATH, "utf8"));
if (!same(allowlist.files || [], BASELINE.files) || !same(allowlist.traits || [], BASELINE.traits)) {
  errors.push("scale allowlist was expanded or changed; only removing baseline entries is allowed");
}
const allowedFiles = new Set(allowlist.files || []);
const allowedTraits = new Set(allowlist.traits || []);

for (const path of walk(ROOT)) {
  const rel = relative(ROOT, path);
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
