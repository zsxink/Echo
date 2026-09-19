#!/usr/bin/env node
// Task 15.1 check: every portable object kind has a directory in the layout.
//
// Why this gate exists (change `restore-user-data-from-library-records`):
// the portable layout used to be *half* implemented — `RecordKind` carried
// object kinds that no production path ever wrote, and nothing compared the
// enum against the directory tree the spec publishes. A newly added kind could
// therefore be silently dropped (no directory → nowhere to write → the object
// only ever lived in the local SQLite, which is exactly how playlists and
// favorites were lost).
//
// Acceptance (tasks 4.5):
//   - every `RecordKind` variant declares a `dir_name()` arm;
//   - every `RecordKind` variant is enumerated by `RecordKind::ALL` (the list
//     the projection/reconciliation passes walk, so a missing entry means the
//     kind is never read back either);
//   - the set of `dir_name()` values is EXACTLY the set of `echo/records/<dir>/`
//     directories the `portable-library-layout` spec publishes — neither a kind
//     without a directory nor a directory without a kind.
//
// The two inputs are read through `ECHO_RECORD_KIND_CHECK_ROOT` (default: the
// repository root) so the injection suite can violate each rule in a scratch
// copy instead of mutating the tree — see injection-suite.mjs.

import { readFileSync } from "node:fs";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

// `new URL(".", import.meta.url)` is this file's directory
// (`scripts/verify/checks/`); three `..` land on the repository root.
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const ROOT = process.env.ECHO_RECORD_KIND_CHECK_ROOT
  ? resolve(process.env.ECHO_RECORD_KIND_CHECK_ROOT)
  : REPO_ROOT;

const SPEC = join(ROOT, "openspec", "specs", "portable-library-layout", "spec.md");
const KINDS = join(ROOT, "crates", "echo-core", "src", "domain", "library.rs");

function fail(msg) {
  process.stderr.write(`FAIL 15.1: ${msg}\n`);
  process.exit(1);
}

function read(path, what) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    fail(`cannot read ${what} at ${path}`);
  }
}

/** The `echo/records/<dir>/` directories published by the spec's layout block. */
function specRecordDirs(text) {
  const fence = text.match(/```text\n([\s\S]*?)```/);
  if (!fence) fail("portable-library-layout spec has no ```text layout block");
  const dirs = new Set();
  for (const m of fence[1].matchAll(/([A-Za-z0-9-]+)\/<uuid-prefix>\//g)) dirs.add(m[1]);
  if (dirs.size === 0) fail("the layout block declares no `records/<dir>/<uuid-prefix>/` entries");
  return dirs;
}

/** The `RecordKind` variants declared by the domain enum. */
function enumVariants(text) {
  const body = text.match(/pub enum RecordKind \{([\s\S]*?)\n\}/);
  if (!body) fail("domain/library.rs declares no `pub enum RecordKind`");
  const variants = [];
  // Strip doc comments so only real variant lines (`    Name,`) remain.
  const stripped = body[1].replace(/^\s*\/\/.*$/gm, "");
  for (const m of stripped.matchAll(/^\s{4}([A-Z][A-Za-z0-9]*),$/gm)) variants.push(m[1]);
  if (variants.length === 0) fail("could not parse any RecordKind variant");
  return variants;
}

/** Slice one item body so its `match` arms cannot be confused with another's. */
function itemBody(text, needle, end) {
  const start = text.indexOf(needle);
  if (start < 0) fail(`domain/library.rs has no \`${needle.trim()}\``);
  const stop = text.indexOf(end, start);
  if (stop < 0) fail(`domain/library.rs's \`${needle.trim()}\` is unterminated`);
  return text.slice(start, stop);
}

function dirNameArms(text) {
  const arms = new Map();
  for (const m of itemBody(text, "fn dir_name", "\n    }").matchAll(
    /Self::([A-Za-z0-9]+)\s*=>\s*"([a-z0-9-]+)"/g,
  )) {
    arms.set(m[1], m[2]);
  }
  return arms;
}

function allKinds(text) {
  return new Set(
    [...itemBody(text, "const ALL", "];").matchAll(/Self::([A-Za-z0-9]+)/g)].map((m) => m[1]),
  );
}

const specDirs = specRecordDirs(read(SPEC, "the portable-library-layout spec"));
const source = read(KINDS, "domain/library.rs");
const variants = enumVariants(source);
const arms = dirNameArms(source);
const enumerated = allKinds(source);

for (const kind of variants) {
  if (!arms.has(kind)) fail(`record kind ${kind} has no dir_name() arm`);
  if (!enumerated.has(kind)) fail(`record kind ${kind} is missing from RecordKind::ALL`);
}
for (const kind of enumerated) {
  if (!variants.includes(kind)) fail(`RecordKind::ALL names ${kind}, which the enum does not declare`);
}

const codeDirs = new Set(arms.values());
for (const dir of codeDirs) {
  if (!specDirs.has(dir)) {
    fail(`record kind directory '${dir}' is absent from the portable-library-layout spec`);
  }
}
for (const dir of specDirs) {
  if (!codeDirs.has(dir)) {
    fail(`the spec publishes echo/records/${dir}/ but no RecordKind maps to it`);
  }
}

process.stdout.write(
  `ok 15.1: ${variants.length} record kinds <-> ${specDirs.size} spec directories ` +
    `(${[...codeDirs].sort().join(", ")})\n`,
);
