#!/usr/bin/env node
// Task 16.2 check — every load attempt resets the progress facts
// (normalize-os-file-open-paths, `desktop-playback`: 加载尝试必须重置进度事实).
//
// Fails (nonzero) unless:
//   1. the actor test suite is green on this host, including the five tests
//      that pin the reset behavior (three Failed entry points, the Loading
//      snapshot, and the mpv-report-driven facts after FileLoaded);
//   2. the reset has a single entry point (`begin_load`) that every load path
//      calls — a fourth load command added later must not silently skip it.
//
// `is_local_media_path` (task 8.3 defense-in-depth) must stay untouched; this
// check runs its test but never relaxes the refusal.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 16.2: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. Behavior: the reset semantics, pinned by the actor tests.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--all-features", "--lib", "--locked", "player::actor::tests"],
  "echo-desktop actor tests",
);
for (const name of [
  "a_rejected_non_local_load_clears_stale_progress_facts",
  "a_library_song_resolve_failure_clears_stale_progress_facts",
  "a_paused_library_song_resolve_failure_clears_stale_progress_facts",
  "entering_loading_starts_without_stale_progress_facts",
  "file_loaded_still_drives_progress_from_mpv_reports",
  "is_local_media_path_classifies_schemes",
]) {
  if (!testOut.includes(`test player::actor::tests::${name} ... ok`)) {
    fail(`actor test not green: ${name}`);
  }
}

// 2. Structure: one reset entry point, called by every load path.
const actor = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor.rs"),
  "utf8",
);
const beginLoadDefs = (actor.match(/fn begin_load\(&mut self\) -> u64/g) || []).length;
if (beginLoadDefs !== 1) {
  fail(`expected exactly one begin_load definition, found ${beginLoadDefs}`);
}
// Three load entry points (two library resolver branches + LoadTemporary) plus
// the definition's own call sites in tests. At minimum the three production
// calls must exist.
const beginLoadCalls = (actor.match(/self\.begin_load\(\);/g) || []).length;
if (beginLoadCalls < 3) {
  fail(`expected begin_load() to be called by every load path, found ${beginLoadCalls} call(s)`);
}
// load_path no longer bumps the generation itself — the reset belongs to
// begin_load, and load_path runs *after* the caller's reset (a duplicate bump
// here would double-increment on every resolved load).
const loadPathBody = actor.slice(
  actor.indexOf("fn load_path(&mut self, path: &Path)"),
  actor.indexOf("fn is_local_media_path("),
);
if (!loadPathBody) {
  fail("could not locate load_path in actor.rs");
}
if (/generation \+= 1/.test(loadPathBody)) {
  fail("load_path bumps the generation itself — the reset must come from begin_load only");
}

process.stdout.write("ok 16.2: every load attempt resets position/duration before loading\n");
