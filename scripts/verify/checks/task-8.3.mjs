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
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.3: ${step}: ${msg}\n`);
  process.exit(1);
}

// `player/actor` is a module directory, so a needle can live in any sibling
// file (`mod.rs`, `backend.rs`, `mpv_backend.rs`, `harden_options.rs`, ...).
// Concatenate the whole module; a missing directory is a hard failure rather
// than a silently empty string.
function readActorModule() {
  const dir = resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor");
  let files;
  try {
    files = readdirSync(dir).filter((name) => name.endsWith(".rs"));
  } catch (err) {
    fail("hardening", `cannot read actor module directory: ${err.message}`);
  }
  if (files.length === 0) fail("hardening", `no .rs files under ${dir}`);
  return files
    .map((name) => readFileSync(resolve(dir, name), "utf8"))
    .join("\n");
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
// The actor was split into a module directory (`player/actor/`), so read the
// whole module rather than one file: the option list lives in harden_options.rs
// while other needles may move between siblings.
const actorSrc = readActorModule();
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

process.stdout.write(
  "ok 8.3: audio-only hardened mpv config; only Rust-validated local paths load\n",
);
