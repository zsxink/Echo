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

// 2. Both OS open paths converge on the FIFO via `deliver_file_open`.
const main = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
// The single-instance callback and RunEvent::Opened must both call the FIFO
// router, never emit directly. The router takes the process-wide supervisor
// plus the raw OS payload (`PathBuf` from argv, decoded path from `open_targets`).
if (!/for path in args\.into_iter\(\)\.skip\(1\) \{\s*deliver_file_open\(app, &startup, PathBuf::from\(path\)\);/.test(main)) {
  fail("single-instance argv path does not route through deliver_file_open");
}
if (!/for path in open_targets::open_targets\(&urls\) \{\s*deliver_file_open\(app, &startup, path\);/.test(main)) {
  fail("RunEvent::Opened path does not route through deliver_file_open");
}
if (!main.includes("RunEvent::Opened") || !main.includes("FILE_OPEN_REQUEST")) {
  fail("main.rs missing the macOS open-event / file-open-event wiring");
}

process.stdout.write("ok 9.2: guaranteed-format file association + open-path unification\n");
