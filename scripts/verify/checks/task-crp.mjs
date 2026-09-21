#!/usr/bin/env node
// Release-pipeline drift gate (ci-release-pipeline capability).
//
// The five requirements of the ci-release-pipeline spec describe behavior that
// only a real tag-driven release run exercises end-to-end (the v0.1.0 Release
// is that evidence). What this gate makes locally repeatable is the structural
// contract behind each requirement: the workflow must declare the triggers, the
// per-platform bundle targets, the artifact assertion + SHA256SUMS, the NOTICE
// upload, and the idempotent publish job. Each CRP scenario resolves to this
// single script (scenario-command dedup runs it once); a drift in any contract
// below fails the whole CRP domain rather than a silent attestation marker.
//
// Fails (nonzero) unless:
//   R01 — release.yml fires on `v*` tags and workflow_dispatch, and pins the
//         version: verify-version.mjs is invoked with the resolved tag, and
//         tauri.conf.json declares a version (tp triggers reject mismatch).
//   R02 — the build matrix covers macOS/Windows/Linux and bundle.targets
//         yields .app+.dmg / .exe / .deb+.AppImage.
//   R03 — expected artifacts are asserted before publish and SHA256SUMS is
//         generated per platform.
//   R04 — the libmpv NOTICE.md is uploaded with the bundle.
//   R05 — a publish job aggregates all artifacts through action-gh-release to
//         the tag's Release (idempotent, generate_release_notes).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const RELEASE_YML = resolve(ROOT, ".github", "workflows", "release.yml");
const TAURI_CONF = resolve(ROOT, "apps", "desktop", "src-tauri", "tauri.conf.json");

function fail(msg) {
  process.stderr.write(`FAIL CRP: ${msg}\n`);
  process.exit(1);
}

function read(path, what) {
  try {
    return readFileSync(path, "utf8");
  } catch (error) {
    fail(`${what} unreadable: ${path} (${error.message})`);
  }
}

const yml = read(RELEASE_YML, "release.yml");
const conf = read(TAURI_CONF, "tauri.conf.json");

// R01 — triggers + version pinning (CRP-R01-S01/02/03).
for (const token of [
  'name: Release',
  'push:',
  'tags:',
  'workflow_dispatch:',
  'verify-version.mjs',
]) {
  if (!yml.includes(token)) fail(`release.yml no longer declares '${token}' (R01 tag/discard/version wiring)`);
}
for (const token of ['"v*"', 'GITHUB_REF_NAME', 'github.event.inputs.tag']) {
  if (!yml.includes(token)) fail(`release.yml no longer declares '${token}' (R01 trigger resolution)`);
}
if (!/tauri\.conf\.json/.test(yml)) fail("release.yml no longer checks the version against tauri.conf.json (R01)");
if (!/"version"\s*:\s*"[0-9]+\.[0-9]+\.[0-9]+"/.test(conf)) fail("tauri.conf.json has no semver version (R01)");

// R02 — three-platform matrix + bundle targets (CRP-R02-S01/02/03).
for (const token of ['macos-latest', 'windows-latest', 'ubuntu-latest']) {
  if (!yml.includes(token)) fail(`release.yml build matrix misses ${token} (R02)`);
}
const targets = conf.match(/"targets"\s*:\s*\[([^\]]*)\]/)?.[1] ?? "";
for (const t of ['dmg', 'nsis', 'appimage', 'deb']) {
  if (!new RegExp(`"${t}"`).test(targets)) fail(`tauri.conf.json bundle.targets lacks ${t} (R02)`);
}

// R03 — artifact assertion before publish + per-platform SHA256SUMS (CRP-R03-S01/02).
if (!yml.includes("Assert expected artifacts exist")) fail("release.yml lost the artifact assertion step (R03)");
for (const token of ["SHA256SUMS", "sha256sum", "if-no-files-found"]) {
  if (!yml.includes(token)) fail(`release.yml no longer declares '${token}' (R03 checksum/integrity)`);
}

// R04 — libmpv license notice ships with the bundle (CRP-R04-S01).
if (!yml.includes("NOTICE.md")) fail("release.yml no longer uploads libmpv NOTICE.md (R04)");

// R05 — publish job aggregates to the tag Release idempotently (CRP-R05-S01/02).
for (const token of [
  "download-artifact",
  "path: artifacts",
  "softprops/action-gh-release@v2",
  "tag_name:",
  "generate_release_notes: true",
]) {
  if (!yml.includes(token)) fail(`release.yml no longer declares '${token}' (R05 publish/aggregate)`);
}

process.stdout.write(
  "ok CRP: release.yml declares tag+manual triggers with version pinning, three-platform targets, artifact assertion + SHA256SUMS, NOTICE upload, idempotent tag Release publish\n",
);