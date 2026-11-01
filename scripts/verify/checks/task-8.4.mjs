#!/usr/bin/env node
// Task 8.4 check: normalised load/file-loaded/end/property/error events and
// foreground 10 Hz / background 1 Hz snapshot throttling.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player` passes, including the
//      `publish_throttled_skips_within_interval_and_emits_after`,
//      `interval_lengths_match_10hz_foreground_and_1hz_background` and
//      `discrete_pause_is_immediate_even_while_property_is_throttled` tests;
//   2. `BackendEvent` covers the required normalised event vocabulary
//      (FileLoaded / Ended / PropertyChanged / Shutdown);
//   3. `cargo clippy` and `cargo fmt` are clean.

import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.4: ${step}: ${msg}\n`);
  process.exit(1);
}

// `player/actor` is a module directory, so a needle can live in any sibling
// file. Concatenate the whole module; a missing directory is a hard failure
// rather than a silently empty string.
function readActorModule() {
  const dir = resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor");
  let files;
  try {
    files = readdirSync(dir).filter((name) => name.endsWith(".rs"));
  } catch (err) {
    fail("source", `cannot read actor module directory: ${err.message}`);
  }
  if (files.length === 0) fail("source", `no .rs files under ${dir}`);
  return files.map((name) => readFileSync(resolve(dir, name), "utf8")).join("\n");
}

const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);
for (const name of [
  "publish_throttled_skips_within_interval_and_emits_after",
  "interval_lengths_match_10hz_foreground_and_1hz_background",
  "discrete_pause_is_immediate_even_while_property_is_throttled",
  "property_event_updates_shared_position",
]) {
  if (!test.stdout.includes(name)) fail("cargo test", `missing test ${name}`);
}

// BackendEvent must cover the required normalised vocabulary.
const actorSrc = readActorModule();
for (const needle of ["FileLoaded", "Ended", "PropertyChanged", "Shutdown"]) {
  if (!actorSrc.includes(needle)) fail("event vocab", `missing BackendEvent::${needle}`);
}

// The foreground/background throttle rates must be 100 ms / 1 s. The background
// rate is written as `from_secs(1)` rather than `from_millis(1000)`; the
// behavioural test above pins both values, this is the structural double-check.
if (
  !actorSrc.includes("Duration::from_millis(100)") ||
  !actorSrc.includes("Duration::from_secs(1)")
) {
  fail("throttle", "missing 100 ms foreground / 1 s background interval definitions");
}

const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.4: normalised events + foreground 10 Hz / background 1 Hz snapshot throttle\n",
);
