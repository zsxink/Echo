#!/usr/bin/env node
// Task 9.5 check: system-trash + reveal adapters (retry policy, irreversibility,
// parent-directory fallback; all entries take SongId/OperationId only).
//
// The OS calls (real `trash`-crate recycle-bin move, real file-manager open)
// are injected backends the composition root wires; this check proves the
// platform policies and the id-only boundary that are valid without an OS:
//   - trash: bounded Windows file-lock retry; `Ok` only on unambiguous success;
//   - reveal: "not locatable" falls back to the parent directory (Linux), never
//     an Unavailable, unless every path fails;
//   - every entry takes SongId/OperationId — the service boundary never leaks an
//     absolute path.
//
// Fails (nonzero) unless those unit tests pass and the binary is clippy-clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.5: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

const trashOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::trash"],
  "trash policy tests",
);
if (!trashOut.includes("test result: ok.")) {
  fail("trash policy tests not green");
}

const revealOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::reveal"],
  "reveal policy tests",
);
if (!revealOut.includes("test result: ok.")) {
  fail("reveal policy tests not green");
}

run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], "echo-app clippy clean");

// The service boundary accepts SongId and returns only a relative path.
// `runtime/services.rs` was split into a `runtime/services/` module; the reveal
// entry now lives in `reveal.rs`. Reading the old path threw ENOENT and made
// this check fail for a reason unrelated to what it guards.
const services = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "runtime", "services", "reveal.rs"),
  "utf8",
);
if (!services.includes("pub fn reveal_song(&self, song: SongId)")) {
  fail("reveal entry no longer takes SongId");
}
if (!/relative_path: record\.path\(\)\.normalized\(\)/.test(services)) {
  fail("reveal result does not return the relative path only");
}

process.stdout.write(
  "ok 9.5: trash retry/irreversibility + reveal parent-fallback policies, id-only boundary; OS backends deferred to composition root\n",
);
