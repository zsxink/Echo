#!/usr/bin/env node
// Release version gate: asserts that the triggering git tag's version matches
// the app version declared in tauri.conf.json, so a Release can never carry
// bundles whose version disagrees with the tag.
//
// Usage:
//   node scripts/release/verify-version.mjs <tag-name> <path-to-tauri.conf.json>
//
// Exit codes:
//   0 — tag parses as v<major>.<minor>.<patch> and matches the config version
//   1 — tag is not a semantic version, or the versions differ (message on stderr)
//
// The tag name may be bare ("0.1.0") or prefixed ("v0.1.0"); both are accepted
// and normalized to the bare form before comparison.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const [, , tagRaw, confPathRaw] = process.argv;

function fail(message) {
  process.stderr.write(`verify-version: ${message}\n`);
  process.exit(1);
}

if (!tagRaw) {
  fail("missing required argument <tag-name> (e.g. v0.1.0)");
}
if (!confPathRaw) {
  fail("missing required argument <path-to-tauri.conf.json>");
}

const bare = tagRaw.startsWith("v") ? tagRaw.slice(1) : tagRaw;
const semver = /^(\d+)\.(\d+)\.(\d+)$/;
if (!semver.test(bare)) {
  fail(
    `tag "${tagRaw}" is not a semantic version (expected v<major>.<minor>.<patch>, e.g. v0.1.0)`,
  );
}

const confPath = resolve(confPathRaw);
let conf;
try {
  conf = JSON.parse(readFileSync(confPath, "utf8"));
} catch (error) {
  fail(`cannot read tauri config "${confPath}": ${error.message}`);
}

const confVersion = conf.version;
if (confVersion !== bare) {
  fail(
    `version mismatch: tag "${tagRaw}" (normalized "${bare}") != tauri.conf.json version "${confVersion}"`,
  );
}

process.stdout.write(`ok: tag ${tagRaw} matches tauri.conf.json version ${confVersion}\n`);