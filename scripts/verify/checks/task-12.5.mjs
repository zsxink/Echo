#!/usr/bin/env node
// Task 12.5 check: 50k synthetic-library benchmark + virtual-DOM bound.
//
// Acceptance (tasks 12.5, design §6 / §14):
//  - A 50,000-song synthetic library: search p95 ≤ 200 ms and view
//    first-screen p95 ≤ 500 ms through the real SQLite query path.
//  - The UI never materializes all rows: the windowed song list renders only a
//    viewport slice (existing SongList test asserts < 100 DOM rows for 50,000).
//  - The benchmark results are saved as a CI artifact (written to a stable
//    .benchmark.json under apps/desktop, uploaded by the workflow).
//
// This runs the `#[ignore]`d bench test explicitly (cargo test -- --ignored
// picks it up) so normal `cargo test` stays fast, and writes the artifact.

import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(msg) {
  process.stderr.write(`FAIL 12.5: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.5: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// --- 1. Rust 50k benchmark (real SQLite query path) ---
const benchOut = run("cargo", [
  "test",
  "-p",
  "echo-core",
  "--all-features",
  "bench_50k_search_and_first_screen_p95_meet_prd_budgets",
  "--",
  "--ignored",
]);

// Extract the reported latencies from the bench's stdout (the test asserts the
// budgets itself; this check also mirrors them into the artifact).
const benchMatch = /search p95 ([\d.]+) ms.*first-screen p95 ([\d.]+) ms/.exec(benchOut);
if (!benchMatch) {
  fail(`benchmark stdout missing expected latency report\n${benchOut}`);
}
const searchP95Ms = Number(benchMatch[1]);
const viewP95Ms = Number(benchMatch[2]);
if (searchP95Ms > 200) fail(`search p95 ${searchP95Ms} ms > 200 ms budget`);
if (viewP95Ms > 500) fail(`view first-screen p95 ${viewP95Ms} ms > 500 ms budget`);

// --- 2. Virtual-DOM bound (viewport slice only for 50k) ---
run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/library/SongList.test.tsx"], APP);

// --- 3. CI artifact: benchmark results ---
const artifact = resolve(APP, "benchmark-50k.json");
writeFileSync(
  artifact,
  `${JSON.stringify({ searchP95Ms, viewP95Ms, songs: 50_000, budgets: { search: 200, firstScreen: 500 } }, null, 2)}\n`,
);
process.stdout.write(`  artifact: ${artifact} (search p95 ${searchP95Ms} ms, view p95 ${viewP95Ms} ms)\n`);

process.stdout.write("ok 12.5: 50k benchmark within PRD budgets; virtual DOM bounded to viewport; result saved as CI artifact\n");