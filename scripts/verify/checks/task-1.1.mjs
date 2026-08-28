#!/usr/bin/env node
// Task 1.1 check: root Cargo workspace with the expected members, and
// echo-core must not depend on Tauri/mpv.
//
// Fails (nonzero) unless `cargo metadata --no-deps` lists exactly the expected
// workspace members and echo-core's manifest declares no Tauri/mpv dependency.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 1.1: ${msg}\n`);
  process.exit(1);
}

const meta = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (meta.status !== 0) fail(`cargo metadata exited ${meta.status}`);

const parsed = JSON.parse(meta.stdout);
const names = parsed.packages.map((p) => p.name).sort();
const expected = ["echo-app", "echo-core", "echo-desktop"].sort();
if (JSON.stringify(names) !== JSON.stringify(expected)) {
  fail(`unexpected workspace members: ${names.join(", ")}`);
}

// The boundary is about real dependency declarations, not prose: scan only
// package names inside [dependencies]/[dev-dependencies]/[build-dependencies]
// (incl. per-target tables). Comments and free text elsewhere must not fail
// the check (e.g. a `testkit` feature note mentioning "mpv").
const coreManifest = readFileSync(
  resolve(ROOT, "crates", "echo-core", "Cargo.toml"),
  "utf8",
);
const dependencyNames = [];
let inDependencySection = false;
for (const rawLine of coreManifest.split(/\r?\n/)) {
  const line = rawLine.trim();
  if (line === "" || line.startsWith("#")) continue;
  const section = line.match(/^\[(.+)\]$/);
  if (section) {
    inDependencySection = /^(dependencies|dev-dependencies|build-dependencies|target\.[^.]+\.dependencies)$/.test(
      section[1],
    );
    continue;
  }
  if (!inDependencySection) continue;
  const name = line.match(/^([A-Za-z0-9_-]+)\s*=/);
  if (name) dependencyNames.push(name[1]);
}
const banned = dependencyNames.filter((name) =>
  /(?:^|lib)(?:tauri|mpv)/i.test(name),
);
if (banned.length > 0) {
  fail(`echo-core declares tauri/mpv dependencies: ${banned.join(", ")}`);
}

process.stdout.write("ok 1.1: workspace members and echo-core dependency boundary\n");
