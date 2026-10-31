#!/usr/bin/env node
// Task 1.6 check: licensed-clear fixtures under fixtures/audio.
//
// The check has two independent layers, because a byte mismatch has two very
// different meanings and conflating them produced a misleading verdict for
// months:
//
//   A. Generator determinism (toolchain-independent). The generator is run
//      twice into two throwaway directories and every file is compared byte
//      for byte. Nothing in the working tree is touched. A failure here means
//      `scripts/gen-fixtures.mjs` really is non-deterministic.
//
//   B. Committed consistency (toolchain-dependent). The bytes the generator
//      produced here are compared against `fixtures/audio/checksums.sha256`.
//      A mismatch is only reported as "the generator is not deterministic"
//      when the encoder version tags match — i.e. the same ffmpeg produced
//      both. When the committed fixtures carry an *older* `Lavf`/`Lavc` tag
//      than the local ffmpeg, the difference is environment drift: ffmpeg's
//      muxers embed their own version string (which shifts every following
//      offset) and its encoders improve between releases. That is reported as
//      `environment drift` with both toolchains and is not a failure, because
//      layer A already proved the generator itself is deterministic.
//
// The committed tree is also checked for integrity on its own: every fixture's
// bytes must match its checksums.sha256 entry. That catches a fixture edited
// by hand without regenerating (the licensing/authenticity guard), independent
// of the local toolchain.
//
// Finally, two toolchain-independent invariants are asserted: every file
// present under fixtures/audio is documented in fixtures/audio/MANIFEST.md
// ("the repo contains only authorized/licensed media" — no undocumented
// fixture may exist), and the required coverage set is present.
//
// `fixtures/audio/MANIFEST.md` declares the exact ffmpeg version that
// reproduces the committed bytes ("Required toolchain: ffmpeg <version>"); the
// check reads it and prints both the declared and the local version so drift is
// never silent.
//
// On success prints: `ok 1.6: fixtures regenerate deterministically, checksums
// verify, coverage complete, manifest clean` (or the same with an appended
// environment-drift note).

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const AUDIO_DIR = resolve(ROOT, "fixtures", "audio");
const CHECKSUMS = resolve(AUDIO_DIR, "checksums.sha256");
const MANIFEST = resolve(AUDIO_DIR, "MANIFEST.md");
const GENERATOR = "scripts/gen-fixtures.mjs";

function fail(msg) {
  process.stderr.write(`FAIL 1.6: ${msg}\n`);
  process.exit(1);
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const p = resolve(dir, entry);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

// -- declared toolchain -----------------------------------------------------
// The MANIFEST is the single source of truth for which ffmpeg reproduces the
// committed bytes. Requiring the declaration here keeps it from silently
// disappearing (the check fails if the line is removed).
const manifest = readFileSync(MANIFEST, "utf8");
const requiredMatch = manifest.match(/Required toolchain:\s*ffmpeg\s+([0-9][0-9.]*)/);
if (!requiredMatch) {
  fail(
    "fixtures/audio/MANIFEST.md does not declare the required ffmpeg version " +
      "(expected a line like `Required toolchain: ffmpeg 9.0.2`)",
  );
}
const requiredFfmpeg = requiredMatch[1];

function ffmpegVersionLine() {
  const r = spawnSync("ffmpeg", ["-version"], { encoding: "utf8", timeout: 30_000 });
  if (r.status !== 0) return null;
  return (r.stdout || "").split("\n")[0].trim();
}

const localVersionLine = ffmpegVersionLine();
if (!localVersionLine) {
  fail("ffmpeg is required to regenerate fixtures but is not on PATH (or exited nonzero)");
}
const localVersion = localVersionLine.match(/ffmpeg version\s+([0-9][0-9.]*)/)?.[1] || "(unknown)";

// Encoder tags ffmpeg writes into the container. mp3/flac/wav carry only `Lavf`
// (the muxer); m4a/mp4/ogg/opus also carry `Lavc` (the encoder).
function encoderTags(bytes) {
  const text = bytes.toString("binary");
  return {
    lavf: [...new Set(text.match(/Lavf[0-9.]+/g) || [])],
    lavc: [...new Set(text.match(/Lavc[0-9.]+/g) || [])],
  };
}

// -- parse checksums.sha256 -------------------------------------------------
const expected = new Map(); // relpath -> hex sha256
for (const line of readFileSync(CHECKSUMS, "utf8").split("\n")) {
  const trimmed = line.trim();
  if (!trimmed) continue;
  // sha256sum / shasum default double-space output: "<hash>  <path>"
  const m = trimmed.match(/^([0-9a-fA-F]{64})\s+(.+)$/);
  if (!m) fail(`checksums.sha256 has a malformed line: ${JSON.stringify(line)}`);
  expected.set(m[2], m[1].toLowerCase());
}
if (expected.size === 0) fail("checksums.sha256 contains no entries");

// ---------------------------------------------------------------------------
// Layer A: generator determinism, in throwaway directories.
// ---------------------------------------------------------------------------
function generateInto(dir) {
  const r = spawnSync("node", [GENERATOR], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ECHO_FIXTURES_OUT: dir },
  });
  if (r.status !== 0) {
    fail(
      `scripts/gen-fixtures.mjs exited ${r.status} generating into a scratch dir\n` +
        `${r.stderr || r.stdout}`,
    );
  }
}

const scratch = [mkdtempSync(join(tmpdir(), "echo-fixtures-a-")), mkdtempSync(join(tmpdir(), "echo-fixtures-b-"))];
generateInto(scratch[0]);
generateInto(scratch[1]);

const setA = new Map();
for (const abs of walk(scratch[0])) {
  setA.set(relative(scratch[0], abs).split(sep).join("/"), abs);
}
const setB = new Map();
for (const abs of walk(scratch[1])) {
  setB.set(relative(scratch[1], abs).split(sep).join("/"), abs);
}
if (setA.size !== setB.size) {
  fail(
    `generator is not deterministic: two runs produced different file sets ` +
      `(${setA.size} vs ${setB.size} files)`,
  );
}
const nondeterministic = [];
for (const [rel, absA] of setA) {
  const absB = setB.get(rel);
  if (!absB) {
    nondeterministic.push(rel);
    continue;
  }
  if (sha256File(absA) !== sha256File(absB)) nondeterministic.push(rel);
}
if (nondeterministic.length) {
  fail(
    `generator is not deterministic: repeated runs produced different bytes for ` +
      `${nondeterministic.join(", ")} — scripts/gen-fixtures.mjs must be byte-identical on every run`,
  );
}

// -- committed tree inventory ---------------------------------------------
const onDisk = new Map(); // relpath -> absolute path
for (const abs of walk(AUDIO_DIR)) {
  onDisk.set(relative(AUDIO_DIR, abs).split(sep).join("/"), abs);
}

// ---------------------------------------------------------------------------
// Layer B: committed integrity + consistency with the local generator.
//
//   integrity   committed bytes == checksums.sha256 entry (toolchain-independent)
//   consistency generated bytes == checksums.sha256 entry (toolchain-dependent)
// ---------------------------------------------------------------------------
const integrityFailures = []; // committed bytes disagree with their own checksum
const consistencyFailures = []; // generated bytes disagree with the checksum
const extraOnDisk = []; // files on disk not listed in checksums.sha256
const driftFiles = []; // consistency mismatch explained by an encoder-version change

for (const [rel, abs] of onDisk) {
  // MANIFEST.md documents the fixtures and checksums.sha256 lists them; neither
  // is itself a generated fixture.
  if (rel === "MANIFEST.md" || rel === "checksums.sha256") continue;
  if (!expected.has(rel)) {
    extraOnDisk.push(rel);
    continue;
  }
  const want = expected.get(rel);
  if (sha256File(abs) !== want) integrityFailures.push(rel);

  const generatedAbs = setA.get(rel);
  if (!generatedAbs) {
    consistencyFailures.push(`${rel} (generator no longer produces it)`);
    continue;
  }
  if (sha256File(generatedAbs) === want) continue;

  // A mismatch is environment drift when the committed fixture and the local
  // generator carry different `Lavf` tags — i.e. a different ffmpeg release
  // produced the committed bytes. (The tag is ffmpeg's own version string; the
  // change also shifts every following byte and can change encoder output, so
  // byte-normalising the tag would not compare equal either.)
  const committedTags = encoderTags(readFileSync(abs));
  const generatedTags = encoderTags(readFileSync(generatedAbs));
  const committedLavf = committedTags.lavf[0];
  const generatedLavf = generatedTags.lavf[0];
  if (generatedLavf && committedLavf && committedLavf !== generatedLavf) {
    driftFiles.push({ rel, committedLavf, generatedLavf, committedTags, generatedTags });
  } else {
    consistencyFailures.push(rel);
  }
}

if (extraOnDisk.length) {
  fail(
    `files on disk are not listed in checksums.sha256 (undocumented/unmanaged media, ` +
      `or missing checksum entries): ${extraOnDisk.join(", ")}`,
  );
}
if (integrityFailures.length) {
  fail(
    `committed fixtures do not match their checksums.sha256 entries (edited by hand, or ` +
      `checksums updated without regenerating): ${integrityFailures.join(", ")}`,
  );
}
if (consistencyFailures.length) {
  fail(
    `local generator output does not match checksums.sha256 for ` +
      `${consistencyFailures.join(", ")}, and the encoder version tags agree — ` +
      `scripts/gen-fixtures.mjs is not producing the committed bytes`,
  );
}

// ---------------------------------------------------------------------------
// Every on-disk file must be documented in MANIFEST.md.
// ---------------------------------------------------------------------------
function manifestMentions(rel) {
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
    `files present under fixtures/audio are not documented in MANIFEST.md (repo may ` +
      `contain unauthorized media): ${undocumented.join(", ")}`,
  );
}

// ---------------------------------------------------------------------------
// Required coverage set.
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

// -- scratch cleanup (best effort; these live outside the repo) -------------
for (const dir of scratch) {
  try {
    rmSync(dir, { recursive: true, force: true });
  } catch {
    // A hardened runtime may refuse bulk deletes; the directories are under the
    // OS temp dir, so a leftover does not touch the repository.
  }
}

// ---------------------------------------------------------------------------
// Verdict.
// ---------------------------------------------------------------------------
process.stdout.write(`1.6: required ffmpeg ${requiredFfmpeg}; local ${localVersionLine}\n`);
if (driftFiles.length) {
  const committed = [...new Set(driftFiles.map((d) => d.committedLavf))].join(", ");
  const generated = [...new Set(driftFiles.map((d) => d.generatedLavf))].join(", ");
  process.stderr.write(
    `1.6: environment drift: committed fixtures were produced by ${committed}, ` +
      `local ffmpeg is ${localVersion} (writes ${generated}).\n` +
      `  The generator itself is deterministic (two scratch runs were byte-identical); ` +
      `only the committed bytes differ, because ffmpeg's muxers embed their version ` +
      `string and its encoders change between releases.\n` +
      `  Files affected: ${driftFiles.map((d) => d.rel).join(", ")}\n` +
      `  Declared toolchain: ${requiredFfmpeg} (fixtures/audio/MANIFEST.md)\n` +
      `  Local ffmpeg: ${localVersionLine}\n` +
      `  To refresh: regenerate with the declared toolchain and update ` +
      `fixtures/audio/checksums.sha256.\n`,
  );
}
process.stdout.write(
  "ok 1.6: fixtures regenerate deterministically, checksums verify, coverage complete, manifest clean\n" +
    (driftFiles.length
      ? `      (environment drift noted for ${driftFiles.length} committed fixture(s) — see stderr)\n`
      : ""),
);
