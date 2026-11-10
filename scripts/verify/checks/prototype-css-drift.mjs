#!/usr/bin/env node
// Generated stylesheet drift gate.
//
// `apps/desktop/src/styles/*.css` are generated from the prototypes by
// `scripts/extract-prototype-css.mjs`; hand-editing them silently diverges the
// app from its visual source of truth. This regenerates into a temporary
// directory and compares byte-for-byte, so any hand edit (or a prototype edit
// that was not followed by a regeneration) fails the gate.
//
// Fix: run `node scripts/extract-prototype-css.mjs` and commit the result.

import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const STYLES = resolve(ROOT, "apps", "desktop", "src", "styles");
const FILES = ["tokens.css", "shell.css", "library.css", "player.css", "responsive.css", "notch.css"];

const out = mkdtempSync(join(tmpdir(), "echo-prototype-css-"));
let failed = false;
try {
  const run = spawnSync("node", ["scripts/extract-prototype-css.mjs", `--out=${out}`], {
    cwd: ROOT,
    encoding: "utf8",
  });
  if (run.status !== 0) {
    process.stderr.write(`FAIL prototype css drift: extraction failed\n${run.stderr}${run.stdout}`);
    process.exit(1);
  }

  const drifted = [];
  for (const name of FILES) {
    let committed;
    try {
      committed = readFileSync(join(STYLES, name), "utf8");
    } catch {
      drifted.push(`${name} (missing)`);
      continue;
    }
    const generated = readFileSync(join(out, name), "utf8");
    if (committed !== generated) drifted.push(name);
  }

  if (drifted.length) {
    failed = true;
    process.stderr.write(
      `FAIL prototype css drift: ${drifted.join(", ")} differ from the prototypes.\n` +
        "Run `node scripts/extract-prototype-css.mjs` and commit the regenerated files; " +
        "do not hand-edit apps/desktop/src/styles/*.css.\n",
    );
  } else {
    process.stdout.write(`ok prototype css drift: ${FILES.length} generated stylesheets match the prototypes\n`);
  }
} finally {
  rmSync(out, { recursive: true, force: true });
}

if (failed) process.exit(1);
