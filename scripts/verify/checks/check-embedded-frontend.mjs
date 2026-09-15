#!/usr/bin/env node
// Embedded-frontend freshness check.
//
// This app has no `devUrl`: `tauri.conf.json` points `frontendDist` at
// `apps/desktop/dist`, so the window loads the frontend that was **compiled
// into the binary**, not the one on disk. The practical consequence is that
// `cargo build` is part of the frontend change, not a separate step — and a
// binary whose embedded bundle predates the last `pnpm build` runs old UI
// while looking completely healthy.
//
// That failure is uniquely expensive: it is indistinguishable from "the
// feature does not work". During the immersive artwork-tint work it produced
// exactly that — a working cover-colour pipeline reported as broken, because
// the running binary still embedded a bundle built before the feature existed.
// No test caught it, because every gate (unit, e2e, vitest) exercises the
// sources through Vite and none of them ever inspects the embedded assets.
//
// Tauri stores each embedded asset under its real path, so the content-hashed
// file names in `dist/index.html` appear as literals inside the binary. If a
// name referenced by the freshly built `index.html` is absent from the binary,
// the binary is serving an older frontend.
//
// Usage:
//   node scripts/verify/checks/check-embedded-frontend.mjs [--bin <path>]
//
// Exits 0 when the binary matches the dist, 1 when it is stale. A missing
// binary is reported as a skip (not a failure) so the check stays usable
// before the first build — the skip is always printed, never silent.

import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const DIST_INDEX = resolve(ROOT, "apps", "desktop", "dist", "index.html");

const CANDIDATE_BINARIES = [
  resolve(ROOT, "target", "debug", "echo"),
  resolve(ROOT, "target", "release", "echo"),
  resolve(ROOT, "apps", "desktop", "src-tauri", "target", "debug", "echo"),
  resolve(ROOT, "apps", "desktop", "src-tauri", "target", "release", "echo"),
];

function resolveBinary() {
  const flag = process.argv.indexOf("--bin");
  if (flag !== -1 && process.argv[flag + 1]) return resolve(process.argv[flag + 1]);
  const existing = CANDIDATE_BINARIES.filter((candidate) => existsSync(candidate));
  if (!existing.length) return null;
  // The newest binary is the one a developer most plausibly just ran.
  return existing.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
}

/** Every content-hashed asset the freshly built entry document references. */
function referencedAssets() {
  if (!existsSync(DIST_INDEX)) return null;
  const html = readFileSync(DIST_INDEX, "utf8");
  const names = new Set();
  for (const match of html.matchAll(/(?:src|href)="\/?(assets\/[^"]+)"/g)) {
    names.add(match[1].split("/").pop());
  }
  return [...names];
}

const binary = resolveBinary();
if (!binary) {
  process.stdout.write(
    "SKIP embedded-frontend: no built binary found — build one (`cargo build -p echo-app`) to check that it matches apps/desktop/dist\n",
  );
  process.exit(0);
}

const assets = referencedAssets();
if (!assets || !assets.length) {
  process.stderr.write(
    `FAIL embedded-frontend: could not read any /assets/ reference from ${DIST_INDEX} — run \`pnpm --dir apps/desktop build\` first\n`,
  );
  process.exit(1);
}

const haystack = readFileSync(binary);
const stale = assets.filter((name) => !haystack.includes(Buffer.from(name, "utf8")));

if (stale.length) {
  process.stderr.write(
    [
      `FAIL embedded-frontend: ${binary} does not embed the current frontend.`,
      `  missing from the binary: ${stale.join(", ")}`,
      `  the binary predates the last \`pnpm --dir apps/desktop build\`.`,
      `  rebuild so the dist is re-embedded: cargo build -p echo-app`,
      `  (a stale binary runs old UI while looking healthy — this is why a`,
      `   shipped frontend change can appear to have done nothing)`,
      "",
    ].join("\n"),
  );
  process.exit(1);
}

process.stdout.write(
  `ok embedded-frontend: ${assets.length} asset(s) in apps/desktop/dist are embedded in ${binary}\n`,
);
