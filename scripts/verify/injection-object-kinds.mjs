#!/usr/bin/env node
// Injection proofs for task 15.1 (object kind <-> layout directory).
//
// Kept out of `injection-suite.mjs` because that file sits right under the
// 1000-line scale ceiling; the suite imports this cluster and spreads it into
// ENTRIES.
//
// Every violation is injected into a scratch copy of the two files 15.1 reads
// (through `ECHO_RECORD_KIND_CHECK_ROOT`) instead of the working tree, so these
// proofs never mutate the repository.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

// This file lives in `scripts/verify/`, so two `..` land on the repository root.
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/**
 * @param {(name: string) => string} fixture  the suite's scratch-dir helper
 * @param {(spec: string, src: string) => {spec: string, src: string}} mutate
 */
function recordKindFixture(fixture, name, mutate) {
  const dir = fixture(name);
  const specPath = join(ROOT, "openspec", "specs", "portable-library-layout", "spec.md");
  const srcPath = join(ROOT, "crates", "echo-core", "src", "domain", "library.rs");
  const mutated = mutate(readFileSync(specPath, "utf8"), readFileSync(srcPath, "utf8"));
  const specOut = join(dir, "openspec", "specs", "portable-library-layout");
  const srcOut = join(dir, "crates", "echo-core", "src", "domain");
  mkdirSync(specOut, { recursive: true });
  mkdirSync(srcOut, { recursive: true });
  writeFileSync(join(specOut, "spec.md"), mutated.spec);
  writeFileSync(join(srcOut, "library.rs"), mutated.src);
  return { env: { ECHO_RECORD_KIND_CHECK_ROOT: dir } };
}

/**
 * @param {(name: string) => string} fixture
 * @returns {Array<object>} ENTRIES-shaped proofs for the 15.1 gate
 */
export function objectKindEntries(fixture) {
  return [
    {
      id: "object-kind-layout/kind-without-directory",
      guard: "a RecordKind whose directory the layout spec never published",
      check: ["node", ["scripts/verify/checks/task-15.1.mjs"]],
      expect: /FAIL 15\.1: record kind directory 'lyrics' is absent from the portable-library-layout spec/,
      baseline: true,
      inject() {
        return recordKindFixture(fixture, "kind-without-directory", (spec, src) => ({
          spec,
          src: src
            .replace("    Tombstone,\n}", "    Tombstone,\n    /// Per-song lyrics.\n    Lyrics,\n}")
            .replace(
              '            Self::Tombstone => "tombstones",',
              '            Self::Tombstone => "tombstones",\n            Self::Lyrics => "lyrics",',
            )
            .replace(
              "        Self::Tombstone,\n    ];",
              "        Self::Tombstone,\n        Self::Lyrics,\n    ];",
            ),
        }));
      },
    },
    {
      id: "object-kind-layout/directory-without-kind",
      guard: "a layout directory no RecordKind maps to (a spec claim with no writer)",
      check: ["node", ["scripts/verify/checks/task-15.1.mjs"]],
      expect: /FAIL 15\.1: the spec publishes echo\/records\/lyrics\/ but no RecordKind maps to it/,
      baseline: true,
      inject() {
        return recordKindFixture(fixture, "directory-without-kind", (spec, src) => ({
          // Insert after the first `records/<dir>/<uuid-prefix>/` row; the last
          // row is a `└──`, so the glyph must be either.
          spec: spec.replace(
            /^(\s*│\s+[├└]── [a-z-]+\/<uuid-prefix>\/.*)$/m,
            "$1\n    │   ├── lyrics/<uuid-prefix>/<lyric-uuid>.json",
          ),
          src,
        }));
      },
    },
    {
      id: "object-kind-layout/kind-missing-from-all",
      guard: "a RecordKind that RecordKind::ALL forgets (never enumerated, never read back)",
      check: ["node", ["scripts/verify/checks/task-15.1.mjs"]],
      expect: /FAIL 15\.1: record kind PlayStats is missing from RecordKind::ALL/,
      baseline: true,
      inject() {
        return recordKindFixture(fixture, "kind-missing-from-all", (spec, src) => ({
          spec,
          src: src.replace("        Self::PlayStats,\n", ""),
        }));
      },
    },
  ];
}
