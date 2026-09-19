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
// Two builds write the *same* `target/debug/Echo`, and only one of them embeds
// anything:
//
//   cargo build -p echo-app   → embeds apps/desktop/dist, no dev server
//   tauri dev                 → injects a devUrl, embeds no frontend at all,
//                               and serves dist from its own static server
//
// So while `tauri dev` owns the binary this check would fail every time, for a
// reason that is not a defect — and a gate that is always red is a gate nobody
// reads. A build that embeds *none* of the current assets but does carry a
// loopback server URL is reported as a skip, named for what it is.
//
// Usage:
//   node scripts/verify/checks/check-embedded-frontend.mjs [--bin <path>]
//
// Exits 0 when the binary matches the dist, 1 when it is stale. A missing
// binary, and a `tauri dev` build, are reported as skips (not failures) so the
// check stays usable outside the run it is meant to guard — the skip is always
// printed, never silent.

import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const DIST_INDEX = resolve(ROOT, "apps", "desktop", "dist", "index.html");

const CANDIDATE_BINARIES = [
  resolve(ROOT, "target", "debug", "Echo"),
  resolve(ROOT, "target", "release", "Echo"),
  resolve(ROOT, "apps", "desktop", "src-tauri", "target", "debug", "Echo"),
  resolve(ROOT, "apps", "desktop", "src-tauri", "target", "release", "Echo"),
];

/** The CLI's static dev server, as a dev build embeds it — or `null`.
 *
 *  Scanned as bytes rather than by decoding a 100 MB binary to a string, and
 *  with a mandatory numeric port so the CSP's `http://cover.localhost` (no
 *  port) cannot be mistaken for it. */
function findDevServerUrl(buffer) {
  for (const prefix of [
    "http://127.0.0.1:",
    "http://localhost:",
    "https://127.0.0.1:",
    "https://localhost:",
  ]) {
    const at = buffer.indexOf(Buffer.from(prefix, "utf8"));
    if (at === -1) continue;
    let end = at + prefix.length;
    while (end < buffer.length && buffer[end] >= 0x30 && buffer[end] <= 0x39) end += 1;
    if (end === at + prefix.length) continue;
    return buffer.subarray(at, end).toString("utf8");
  }
  return null;
}

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
const missing = assets.filter((name) => !haystack.includes(Buffer.from(name, "utf8")));
const embedded = assets.length - missing.length;

// Some names match but not all: this binary embeds a frontend, just not this
// one. There is no reading of that other than "stale".
if (missing.length && embedded) {
  process.stderr.write(
    [
      `FAIL embedded-frontend: ${binary} does not embed the current frontend.`,
      `  missing from the binary: ${missing.join(", ")}`,
      `  (${embedded} of ${assets.length} present — an older bundle of the same app)`,
      `  the binary predates the last \`pnpm --dir apps/desktop build\`.`,
      `  rebuild so the dist is re-embedded: cargo build -p echo-app`,
      `  (a stale binary runs old UI while looking healthy — this is why a`,
      `   shipped frontend change can appear to have done nothing)`,
      "",
    ].join("\n"),
  );
  process.exit(1);
}

// Nothing matches. Either the whole bundle rotated (stale) or this binary was
// never meant to embed one, because `tauri dev` rebuilds the same path with a
// devUrl and serves the frontend from its own server instead.
if (missing.length) {
  const devServer = findDevServerUrl(haystack);
  if (devServer) {
    process.stdout.write(
      [
        `SKIP embedded-frontend: ${binary} is a \`tauri dev\` build.`,
        `  it carries the dev server URL ${devServer} and embeds no frontend: the`,
        `  window loads apps/desktop/dist from that server, not from the binary.`,
        `  This check guards the *embedded* build, so compare like with like: stop`,
        `  \`tauri dev\`, run \`cargo build -p echo-app\`, then re-run this check.`,
        "",
      ].join("\n"),
    );
    process.exit(0);
  }
  process.stderr.write(
    [
      `FAIL embedded-frontend: ${binary} embeds no frontend asset at all.`,
      `  none of ${assets.join(", ")} appear in it, and it carries no dev server URL,`,
      `  so it is a stale embedded build rather than a \`tauri dev\` one.`,
      `  rebuild: cargo build -p echo-app`,
      "",
    ].join("\n"),
  );
  process.exit(1);
}

process.stdout.write(
  `ok embedded-frontend: ${assets.length} asset(s) in apps/desktop/dist are embedded in ${binary}\n`,
);
