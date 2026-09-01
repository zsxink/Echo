#!/usr/bin/env node
// Task 8.8 check: seek / volume / mute + recent-non-zero mute restore +
// command-failure authoritative rollback keep the UI snapshot consistent for
// both FakePlayer and the real-mpv actor path.
//
// Fails unless:
//   1. `cargo test -p echo-desktop --lib player` passes the seek/volume/mute
//      tests (coordinator + fake + actor-loop over the scripted backend);
//   2. the coordinator exposes the public seek / set_volume / toggle_mute
//      surface (the composition root the UI/platform layer calls);
//   3. the actor implements the recent non-zero volume restore and the
//      authoritative rollback (a `write_property -> bool`; a rejected write
//      leaves the snapshot untouched);
//   4. FakePlayer mirrors those semantics (last_nonzero_volume + a server-side
//      property-failure knob) so its snapshots match real mpv;
//   5. `cargo clippy` and `cargo fmt` are clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(step, msg) {
  process.stderr.write(`FAIL 8.8: ${step}: ${msg}\n`);
  process.exit(1);
}

// 1. Player tests (coordinator + fake + actor-loop rollback)
const test = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "player"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (test.status !== 0) fail("player tests", `${test.stdout}\n${test.stderr}`);
for (const name of [
  "seek_updates_snapshot_through_coordinator",
  "set_volume_clamps_and_clears_mute_through_coordinator",
  "unmute_restores_last_nonzero_volume_through_coordinator",
  "rejected_property_leaves_snapshot_authoritative", // coordinator
  "unmute_restores_last_nonzero_volume", // fake
  "rejected_property_write_leaves_snapshot_unchanged", // fake
  "actor_seek_publishes_position_and_writes_property", // actor-loop (real-mpv path)
  "actor_toggle_mute_remembers_and_restores_non_zero_volume",
  "actor_rejected_property_leaves_snapshot_authoritative", // actor rollback
]) {
  if (!test.stdout.includes(name)) fail("player tests", `missing test ${name}`);
}

// 2. Coordinator public surface (composition root the UI calls)
const coordSrc = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "player", "coordinator.rs"),
  "utf8",
);
for (const needle of ["pub fn seek", "pub fn set_volume", "pub fn toggle_mute"]) {
  if (!coordSrc.includes(needle)) fail("coordinator surface", `missing ${needle}`);
}

// 3. Actor: recent non-zero volume + authoritative rollback
const actorSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "actor.rs"), "utf8");
for (const needle of [
  "last_nonzero_volume",               // mute remembers the recent non-zero value
  "fn write_property(&mut self, prop: BackendProperty) -> bool", // rejected => false
  "if self.backend.write_property",    // commit only on accepted write
]) {
  if (!actorSrc.includes(needle)) fail("actor rollback", `missing ${needle}`);
}

// 4. FakePlayer mirrors the actor so its snapshots match real mpv
const fakeSrc = readFileSync(resolve(ROOT, "crates", "echo-desktop", "src", "player", "fake.rs"), "utf8");
for (const needle of [
  "last_nonzero_volume",                    // same mute-restore semantics
  "pub fn fail_next_property",              // server-side property-failure knob
  "guard.snapshot.muted = guard.snapshot.muted && clamped == 0.0", // mirror actor's SetVolume
]) {
  if (!fakeSrc.includes(needle)) fail("fake mirror", `missing ${needle}`);
}

// 5. Clippy and fmt
const clippy = spawnSync(
  "cargo",
  ["clippy", "-p", "echo-desktop", "--all-targets", "--", "-D", "warnings"],
  { cwd: ROOT, encoding: "utf8" },
);
if (clippy.status !== 0) fail("clippy", `${clippy.stdout}\n${clippy.stderr}`);

const fmt = spawnSync("cargo", ["fmt", "--all", "--", "--check"], { cwd: ROOT, encoding: "utf8" });
if (fmt.status !== 0) fail("fmt", `${fmt.stdout}\n${fmt.stderr}`);

process.stdout.write(
  "ok 8.8: seek/volume/mute + recent-non-zero restore + command-failure rollback keep snapshot consistent\n",
);
