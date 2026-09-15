#!/usr/bin/env node
// Task 13.2 check: native end-to-end temp-library flow over the REAL
// composition root, exercising scan→search→favorite→playlist→import→
// delete/undo→restart and asserting data/UUID/queue/preference consistency
// across the restart boundary.
//
// The driver is `crates/echo-desktop/tests/native_e2e.rs`:
//   - real `assemble()` + `AppServices` (same services the Tauri shell wraps),
//   - real SQLite + root-constrained fs over a temp library seeded with the
//     licensed audio fixtures,
//   - scripted dialog boundary (root + import) so the flow is hermetic,
//   - a second `AppServices` re-assembly (the "restart") asserting the active
//     root identity, favorites, playlist, imported-song availability and
//     no-duplicate-UUID are unchanged.
// This check runs that suite plus the real-libmpv smoke and the coordinator
// queue-restore tests, and records versions into an evidence file.
//
// On a non-macOS host the check still runs (the driver is host-agnostic); the
// three-OS smoke actuals are the 13.5 manual matrix. Here we prove the seam.

import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const ARTIFACTS = resolve(ROOT, "artifacts");

function fail(msg) {
  process.stderr.write(`FAIL 13.2: ${msg}\n`);
  process.exit(1);
}
function run(command, args, cwd = ROOT) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 13.2: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`);
    process.exit(1);
  }
  return `${r.stdout || ""}${r.stderr || ""}`;
}

// 1. The native temp-library flow + restart consistency.
run("cargo", ["test", "-p", "echo-desktop", "--test", "native_e2e", "--all-features"]);

// 2. Play session restore (won't auto-play) + real libmpv smoke: the same
//    primitives the flow's play step relies on.
run("cargo", ["test", "-p", "echo-desktop", "--all-features", "player::coordinator::tests::"]);
run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--test", "player_smoke"]);

// 3. Evidence: versions + what ran.
mkdirSync(ARTIFACTS, { recursive: true });
const report = resolve(ARTIFACTS, "native-e2e-13.2.txt");
const rustc = run("rustc", ["--version"]).trim();
const cargoVer = run("cargo", ["--version"]).trim();
const lines = [
  "Echo 0.1.0 native end-to-end evidence (task 13.2)",
  `date: ${new Date().toISOString()}`,
  `host: ${process.platform} ${process.arch}`,
  `rustc: ${rustc}`,
  `cargo: ${cargoVer}`,
  "",
  "Driven flow (temp library):",
  "  scan → search → favorite → playlist → import → delete/undo → restart",
  "",
  "Assertions (native_e2e.rs):",
  "  - scan processes the seeded fixtures; search/listing resolve them",
  "  - favorite is set and survives the restart",
  "  - playlist created + member added and survives the restart",
  "  - external file imported via the dialog boundary yields a normal SongId",
  "  - delete + undo restores the favorited song (favorite + playlist intact)",
  "  - active root identity is stable across the restart",
  "  - no duplicate UUID after restart",
  "",
  "Suites run: echo-desktop native_e2e (1) · coordinator (player queue/session) · player_smoke (real libmpv)",
  "",
  "Note: three-platform data/UUID/queue/preference consistency actuals are the",
  "13.5 manual matrix; this host (macOS) runs the seam automatically.",
];
writeFileSync(report, lines.join("\n") + "\n");

process.stdout.write(`ok 13.2: native temp-library E2E (scan→…→delete/undo→restart) passes; evidence at ${report}\n`);