#!/usr/bin/env node
// The original sync-foundation acceptance asserted that sync had no operable
// entry. The LAN sync change supersedes that boundary. Keep this archived
// scenario useful by checking that the now-operable IPC surface stays typed,
// generated from Rust, and covered by the sync runtime tests.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(message) {
  process.stderr.write(`FAIL 3.14: ${message}\n`);
  process.exit(1);
}

function run(args, expectedTest) {
  const result = spawnSync("cargo", args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${args.join(" ")} exited ${result.status}\n${result.stderr || result.stdout}`);
  }
  const output = `${result.stdout || ""}${result.stderr || ""}`;
  if (expectedTest && !output.includes(`${expectedTest} ... ok`)) {
    fail(`expected test '${expectedTest}' did not run successfully`);
  }
}

const bridgePath = resolve(ROOT, "apps", "desktop", "src", "bridge", "index.ts");
const generatedPath = resolve(ROOT, "apps", "desktop", "src", "ipc", "ipc-types.generated.ts");
const bridge = readFileSync(bridgePath, "utf8");
const generated = readFileSync(generatedPath, "utf8");
if (!bridge.includes("SyncCommandArguments") || !bridge.includes("extends SyncCommandArguments")) {
  fail("desktop bridge no longer uses the generated sync-command argument contract");
}
for (const command of ["sync_status", "host_start", "sync_now", "request_sync_pairing"]) {
  if (!generated.includes(`readonly ${command}:`)) {
    fail(`generated sync IPC contract is missing '${command}'`);
  }
}

run([
  "test",
  "-p",
  "echo-desktop",
  "--all-features",
  "ipc::generate::tests::committed_generated_file_is_in_sync_with_the_generator",
], "committed_generated_file_is_in_sync_with_the_generator");
run(["test", "-p", "echo-desktop", "--all-features", "sync_runtime::"]);
run(["test", "-p", "echo-core", "--all-features", "sync_payloads_carry_no_absolute_paths"]);

process.stdout.write("ok 3.14: LAN sync IPC stays generated, typed, and covered by runtime/privacy tests\n");
