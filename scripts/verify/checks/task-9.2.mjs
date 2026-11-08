#!/usr/bin/env node
// Task 9.2 check: guaranteed-format file association + OS open-path unification.
//
// The shell half of 9.2 (the playback layer owns SongId/temporary-item
// resolution, task 11.7):
//   1. tauri.conf.json registers exactly the guaranteed AudioFormat families
//      (mp3/flac/m4a/ogg/opus/wav) — enforced by a Rust drift test;
//   2. macOS `RunEvent::Opened` and single-instance argv both route through the
//      same FIFO in main.rs, so the platform arg shims converge on one path.
//
// Fails (nonzero) unless the file-association drift test is green and both OS
// open paths in the shell go through `deliver_file_open`.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.2: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. File-association registration covers exactly the guaranteed formats.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "tauri_conf_file_associations_cover_the_guaranteed_formats"],
  "file-association drift test",
);
if (!testOut.includes("tauri_conf_file_associations_cover_the_guaranteed_formats ... ok")) {
  fail("file-association drift test not green");
}

// 2. Both OS open paths deliver ONE batch through the FIFO router. Delivering
// per path emits N events and makes the frontend replace the queue N times,
// leaving only the last file (#117 / DP-R07-S08).
const shellRoot = resolve(ROOT, "apps", "desktop", "src-tauri", "src");
const main = readFileSync(resolve(shellRoot, "main.rs"), "utf8");
const bootstrap = readFileSync(resolve(shellRoot, "bootstrap.rs"), "utf8");
const fileOpenGate = readFileSync(resolve(shellRoot, "file_open_gate.rs"), "utf8");
const batchCalls = (bootstrap.match(/deliver_file_opens\(app, &startup/g) || []).length;
if (batchCalls < 2) {
  fail("both OS open paths must deliver one batch through deliver_file_opens");
}
if (/for path in [^{]*\{\s*deliver_file_open\(app, &startup/s.test(bootstrap)) {
  fail("per-path file-open delivery rebuilds the queue once per file (#117)");
}
if (!bootstrap.includes("RunEvent::Opened") || !fileOpenGate.includes("FILE_OPEN_REQUEST")) {
  fail("desktop shell missing the macOS open-event / file-open-event wiring");
}
void main;

process.stdout.write("ok 9.2: guaranteed-format file association + open-path unification\n");
