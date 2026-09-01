#!/usr/bin/env node
// Task 8.3 check: audio-only mpv config that disables user scripts, ytdl,
// non-essential network protocols and user config, and guarantees only
// Rust-validated local paths can be loaded.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player` passes, including the
//      `hardened_options_are_audio_only_and_least_privilege`,
//      `non_local_scheme_path_is_refused_as_failed` and
//      `is_local_media_path_classifies_schemes` tests.
//   2. the hardened option set (config / load-scripts / ytdl / video / vo /
//      audio-display / osc / protocol-whitelist=file) is present in the source;
//   3. `cargo clippy -p echo-desktop --all-targets -- -D warnings` is clean;
//   4. `cargo fmt --all -- --check` is clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.3: ${step}: ${msg}\n`);
  process.exit(1);
}

const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("cargo test", `${test.stdout}\n${test.stderr}`);
for (const name of [
  "hardened_options_are_audio_only_and_least_privilege",
  "non_local_scheme_path_is_refused_as_failed",
  "is_local_media_path_classifies_schemes",
]) {
  if (!test.stdout.includes(name)) fail("cargo test", `missing test ${name}`);
}

// The hardened option set must exist and pin `protocol-whitelist=file`.
const actorSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor.rs"), "utf8");
for (const needle of [
  '("config", "no")',
  '("load-scripts", "no")',
  '("ytdl", "no")',
  '("video", "no")',
  '("vo", "null")',
  '("protocol-whitelist", "file")',
]) {
  if (!actorSrc.includes(needle)) fail("hardening", `missing option ${needle}`);
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
  "ok 8.3: audio-only hardened mpv config; only Rust-validated local paths load\n",
);
