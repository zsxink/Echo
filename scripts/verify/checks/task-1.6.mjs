#!/usr/bin/env node
// Task 1.6 check: licensed-clear fixtures under fixtures/audio.
//
// Fails (nonzero) unless:
//   1. Re-running `node scripts/gen-fixtures.mjs` is deterministic and the
//      on-disk bytes still match every entry in fixtures/audio/checksums.sha256
//      (SHA-256 computed with Node's crypto, no external sha256sum/shasum).
//   2. Every file that exists under fixtures/audio is documented in
//      fixtures/audio/MANIFEST.md ("the repo contains only authorized/licensed
//      media" guard — no undocumented fixture may exist).
//   3. The required coverage set is present: one of each of the six guaranteed
//      formats, an mp4 with an audio track, an mp4 with no audio track, a
//      corrupted container, a synced LRC, a plain-text lyrics file, and the
//      cover PNG.
//
// On success prints: `ok 1.6: fixtures regenerate deterministically, checksums
// verify, coverage complete, manifest clean`.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const AUDIO_DIR = resolve(ROOT, "fixtures", "audio");
const CHECKSUMS = resolve(AUDIO_DIR, "checksums.sha256");
const MANIFEST = resolve(AUDIO_DIR, "MANIFEST.md");

function fail(msg) {
  process.stderr.write(`FAIL 1.6: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Step 1: regenerate and verify checksums via Node crypto.
// ---------------------------------------------------------------------------
const regen = spawnSync("node", ["scripts/gen-fixtures.mjs"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (regen.status !== 0) {
  fail(`scripts/gen-fixtures.mjs exited ${regen.status}\n${regen.stderr || regen.stdout}`);
}

// -- parse checksums.sha256 -------------------------------------------------
const cs = readFileSync(CHECKSUMS, "utf8");
const expected = new Map(); // relpath -> hex sha256
const lines = cs.split("\n");
for (const line of lines) {
  const trimmed = line.trim();
  if (!trimmed) continue;
  // sha256sum / shasum default double-space output: "<hash>  <path>"
  const m = trimmed.match(/^([0-9a-fA-F]{64})\s+(.+)$/);
  if (!m) fail(`checksums.sha256 has a malformed line: ${JSON.stringify(line)}`);
  expected.set(m[2], m[1].toLowerCase());
}
if (expected.size === 0) fail("checksums.sha256 contains no entries");

// -- walk every file under fixtures/audio -----------------------------------
function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const p = resolve(dir, entry);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}
const onDisk = new Map(); // relpath -> absolute path
for (const abs of walk(AUDIO_DIR)) {
  const rel = relative(AUDIO_DIR, abs).split(sep).join("/");
  onDisk.set(rel, abs);
}

// -- verify checksums against on-disk bytes --------------------------------
const extraOnDisk = []; // files on disk not listed in checksums.sha256
for (const [rel, abs] of onDisk) {
  // MANIFEST.md documents the fixtures and is not itself a generated fixture;
  // checksums.sha256 lists the fixtures. Neither is checksummed against itself.
  if (rel === "MANIFEST.md" || rel === "checksums.sha256") continue;
  if (!expected.has(rel)) {
    extraOnDisk.push(rel);
    continue;
  }
  const digest = createHash("sha256").update(readFileSync(abs)).digest("hex");
  const want = expected.get(rel);
  if (digest !== want) {
    fail(
      `checksum mismatch for ${rel}: generator is not deterministic (got ${digest}, expected ${want}) — ` +
        "scripts/gen-fixtures.mjs must produce byte-identical output on every run",
    );
  }
}
if (extraOnDisk.length) {
  fail(
    `files on disk are not listed in checksums.sha256 (undocumented/unmanaged media, or missing checksum entries): ` +
      extraOnDisk.join(", "),
  );
}

// ---------------------------------------------------------------------------
// Step 2: every on-disk file must be documented in MANIFEST.md.
// ---------------------------------------------------------------------------
const manifest = readFileSync(MANIFEST, "utf8");
function manifestMentions(rel) {
  // The manifest has one Markdown table row per fixture; match on the exact
  // relative path. Also allow a reference via a `` `path` `` code span or a
  // plain `path` near the start of a line.
  return (
    manifest.includes(`\`${rel}\``) ||
    new RegExp(`(^|[\\s|])${rel.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}([\\s|]|$)`).test(manifest)
  );
}
const undocumented = [];
for (const rel of onDisk.keys()) {
  if (rel === "checksums.sha256" || rel === "MANIFEST.md") continue;
  if (!manifestMentions(rel)) undocumented.push(rel);
}
if (undocumented.length) {
  fail(
    `files present under fixtures/audio are not documented in MANIFEST.md (repo may contain unauthorized media): ` +
      undocumented.join(", "),
  );
}

// ---------------------------------------------------------------------------
// Step 3: required coverage set.
// ---------------------------------------------------------------------------
const HAVE = (ext) => [...onDisk.keys()].some((r) => r.toLowerCase().endsWith(ext));

const missing = [];
const audioExtensions = [".mp3", ".flac", ".m4a", ".ogg", ".opus", ".wav"];
for (const ext of audioExtensions) {
  if (!HAVE(ext)) missing.push(`a ${ext} file`);
}
if (!HAVE(".mp4") || ![...onDisk.keys()].some((r) => r.endsWith(".mp4") && /video/i.test(r))) {
  missing.push("an mp4 with an audio track (tone-short-video.mp4)");
}
if (!HAVE("no-audio.mp4")) {
  missing.push("an mp4 with no audio track (no-audio.mp4)");
}
if (![...onDisk.keys()].some((r) => /corrupt|broken|damaged/i.test(r))) {
  missing.push("a corrupted container fixture (tone-corrupted.mp3)");
}
if (!HAVE(".lrc")) {
  missing.push("a synced .lrc lyrics file");
}
if (!HAVE(".txt")) {
  missing.push("a plain-text lyrics file");
}
if (![...onDisk.keys()].some((r) => /cover/.test(r) && r.toLowerCase().endsWith(".png"))) {
  missing.push("the cover PNG (cover-256.png)");
}
if (missing.length) {
  fail(`required fixtures missing: ${missing.join(", ")}`);
}

process.stdout.write(
  "ok 1.6: fixtures regenerate deterministically, checksums verify, coverage complete, manifest clean\n",
);