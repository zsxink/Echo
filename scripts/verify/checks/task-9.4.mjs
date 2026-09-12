#!/usr/bin/env node
// Task 9.4 check: media-control seam (macOS Now Playing / Windows SMTC /
// Linux MPRIS).
//
// The OS-specific adapters are platform-crate + composition-root work (each
// needs its own OS and a live coordinator); the platform-agnostic contract is
// proven here: the per-OS key-name decoder, the exactly-one coarse command
// mapping, and the explicit degrade path (no-op sink → window/frontend control)
// so an unavailable capability never silently drops a press.
//
// Fails (nonzero) unless the media_control unit tests pass and the binary stays
// clippy-clean.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.4: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::media_control"],
  "media_control tests",
);
if (!testOut.includes("test result: ok.")) {
  fail("media_control tests not green");
}

run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], "echo-app clippy clean");

process.stdout.write(
  "ok 9.4: media-control seam (key decode, one-command mapping, degrade sink); OS adapters deferred to composition root\n",
);
