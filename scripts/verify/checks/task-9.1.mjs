#!/usr/bin/env node
// Task 9.1 check: productized single-instance + initialization request FIFO.
//
// The shell wires both OS file-open paths (single-instance argv and
// RunEvent::Opened) through the StartupSupervisor FIFO so that file-opens
// received while the runtime is initializing are stashed and drained once
// ready — never silently dropped, and handled only by the already-running
// main instance.
//
// Fails (nonzero) unless:
//   1. the `echo-app` binary compiles and is clippy-clean on this host;
//   2. the echo-desktop runtime FIFO tests pass (bounded, keeps-newest,
//      drains-in-order, ready-drain and later-opens-immediate);
//   3. the shell actually routes both open paths through the supervisor
//      (grep-level structural guard against a regression to a direct,
//      un-FIFO'd `app.emit`).

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.1: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. The thin binary stays thin but now wires the runtime FIFO: it must
//    compile and be clippy-clean.
run("cargo", ["check", "-p", "echo-app", "--locked"], "echo-app compiles");
run(
  "cargo",
  ["clippy", "-p", "echo-app", "--", "-D", "warnings"],
  "echo-app clippy clean",
);

// 2. The FIFO semantics backing task 9.1 are proven by the runtime tests.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "runtime::"],
  "echo-desktop runtime tests",
);
for (const name of [
  "supervisor_sequence_resolves_gate_and_drains_pending_opens",
  "pending_open_fifo_is_bounded_and_keeps_newest",
  "gate_kind_maps_recovery_states",
]) {
  if (!testOut.includes(`test runtime::tests::${name} ... ok`)) {
    fail(`runtime test not green: ${name}`);
  }
}

// 3. Structural guard: the shell routes both OS open paths through the
//    supervisor's FIFO instead of emitting directly.
const main = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
const required = [
  "StartupSupervisor",
  "receive_file_open",
  "on_ready",
  "deliver_file_open",
  "drain_pending_opens",
  "app://file-open-request",
];
for (const token of required) {
  if (!main.includes(token)) {
    fail(`main.rs no longer routes through '${token}'`);
  }
}
// A regression where the single-instance callback bypasses the FIFO and emits
// directly would leave an unguarded `app.emit(FILE_OPEN_REQUEST, ...)` on the
// argv path. Both open paths must go through `deliver_file_open`, so the only
// direct emits to that event live in `deliver_file_open`/`drain_pending_opens`.
const directEmits = (main.match(/app\.emit\(FILE_OPEN_REQUEST/g) || []).length;
if (directEmits > 2) {
  fail(`unexpected direct file-open emits in shell: ${directEmits}`);
}

process.stdout.write("ok 9.1: single-instance + init FIFO wiring, runtime FIFO tests green\n");
