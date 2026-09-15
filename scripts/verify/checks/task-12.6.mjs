#!/usr/bin/env node
// Task 12.6 check: scan/hash/tag/cover worker CPU/memory/cancel stress.
//
// Acceptance (tasks 12.6, design §6.4 / §7 / §18):
//  - The default scan parse worker concurrency is `min(CPU, 4)`, never more;
//    a bounded pool holds that bound under medium-scale pressure.
//  - Cancelling mid-scan never marks anything missing and never corrupts the
//    committed batch (truthful terminal state).
//  - The tag / lyrics / cover input limits (4 KiB / 2 MiB / 20 MiB) and the
//    cover cache capacity (256 MiB GC target) are enforced and tested.
//
// Runs the dedicated stress/limit tests in echo-core (via the same
// cargo-test filter the other checks use) plus the whole scan/recover test
// block, then clippy + fmt on echo-core.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 12.6: ${msg}\n`);
  process.exit(1);
}
function run(command, args) {
  const r = spawnSync(command, args, { cwd: ROOT, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.6: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

for (const testName of [
  "default_worker_concurrency_is_min_cpu_4",
  "stress_scan_bounded_hashes_and_cancels_without_corrupting_songs",
  "bounded_workers_never_exceed_the_configured_bound",
  "input_limits_skip_assets_but_keep_song_fields",
  "cover_cache_gc_keeps_referenced_assets",
]) {
  // Same single-name subprocess pattern the manifest's other check scripts
  // use: `cargo test <filter>` returns 0 only when the filter matches (it
  // errors with "unexpected argument" if the name doesn't exist).
  const r = spawnSync(
    "cargo",
    ["test", "-p", "echo-core", "--all-features", testName],
    { cwd: ROOT, encoding: "utf8" },
  );
  if (r.status !== 0) {
    fail(`test '${testName}' exited ${r.status}\n${r.stdout}\n${r.stderr}`);
  }
  const combined = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.stdout && !/test result: ok\. (1|[2-9]) passed/.test(combined)) {
    fail(`test '${testName}' reported a failure\n${combined}`);
  }
}

// The real limits must be pinned in the source (not only in tests).
run("node", ["-e", `
const fs = require("fs");
const meta = fs.readFileSync("crates/echo-core/src/infrastructure/metadata/mod.rs", "utf8");
if (!meta.includes("tag_field: 4 * 1024")) process.exit(1);
if (!meta.includes("lyrics: 2 * 1024 * 1024")) process.exit(1);
if (!meta.includes("cover: 20 * 1024 * 1024")) process.exit(1);
const scan = fs.readFileSync("crates/echo-core/src/application/scan.rs", "utf8");
if (!/.worker_threads: std::thread::available_parallelism/.test(scan)) process.exit(1);
const cover = fs.readFileSync("crates/echo-core/src/infrastructure/metadata/cover.rs", "utf8");
if (!cover.includes("max_capacity: 256 * 1024 * 1024")) process.exit(1);
process.stdout.write("limits pinned ok\\n");
`]);

// Clippy + fmt on echo-core (regression gate for the new stress code).
run("cargo", ["clippy", "-p", "echo-core", "--all-targets", "--all-features", "--", "-D", "warnings"]);
run("cargo", ["fmt", "--all", "--", "--check"]);

process.stdout.write("ok 12.6: scan/hash/tag/cover worker stress verified (min(CPU,4) default, cancel-safe, input limits + cache capacity enforced)\n");