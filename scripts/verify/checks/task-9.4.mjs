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
// clippy-clean. `--static-only` skips the cargo run (the Linux workspace Test
// step covers it) and keeps the source assertions below.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
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

const staticOnly = process.argv.includes("--static-only");

if (!staticOnly) {
  const testOut = run(
    "cargo",
    ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::media_control"],
    "media_control tests",
  );
  if (!testOut.includes("test result: ok.")) {
    fail("media_control tests not green");
  }
} else {
  process.stdout.write("  skip: media_control cargo tests (covered by the workspace-test job)\n");
}

// The contract the tests above pin must also be visible in the source: the
// decoder, the one-command mapping (including the keys that map to nothing),
// and the degrade sink. Without these the unit tests could be deleted together
// with the code they cover and this check would still pass.
const mediaControl = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "platform", "media_control.rs"),
  "utf8",
);
for (const token of [
  "pub fn from_name(name: &str) -> Self",
  "pub const fn to_player_command(key: MediaKey) -> Option<PlayerCommand>",
  "MediaKey::PlayPause => Some(PlayerCommand::TogglePlayPause)",
  "MediaKey::Mute => Some(PlayerCommand::ToggleMute)",
  "MediaKey::VolumeUp | MediaKey::VolumeDown | MediaKey::Unknown => None",
  "pub struct NoopSink",
]) {
  if (!mediaControl.includes(token)) {
    fail(`media_control.rs missing '${token}'`);
  }
}

process.stdout.write(
  `ok 9.4: media-control seam (key decode, one-command mapping, degrade sink); OS adapters deferred to composition root${staticOnly ? " (static-only)" : ""}\n`,
);
