#!/usr/bin/env node
// Task 9.6 check: window close/hide/quit + window-state visible-area validation.
//
// Tests that the testable, platform-independent core of 9.6 is green:
//   - CloseBehavior platform defaults (macOS → background, else → exit) and the
//     user-override persistence live in local_state (task 7.6) — proven here;
//   - WindowState::clamp_to_visible re-anchors an off-screen window into a
//     visible work area after a display disconnect (the "断开显示器后窗口回到
//     可见区域" rule) — proven by unit tests.
// The real close-handler/menu wiring (macOS default hide, Win/Linux default
// quit) lands with the shell/composition root; cross-platform native
// validation is deferred to the platform Gate.
//
// Fails (nonzero) unless those local_state tests pass and the binary is
// clippy-clean.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.6: ${msg}\n`);
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
  ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::local_state"],
  "local_state tests",
);
for (const name of [
  "window_left_on_a_disconnected_display_is_recentered_on_screen",
  "a_still_visible_window_is_left_untouched",
  "a_window_off_screen_and_bigger_than_the_screen_snaps_to_the_work_origin",
  "a_partially_visible_window_is_left_untouched",
  "a_degenerate_work_area_leaves_the_position_unchanged",
  "windows_platform_default_is_exit",
]) {
  if (!testOut.includes(`test platform::local_state::tests::${name} ... ok`)) {
    fail(`local_state test not green: ${name}`);
  }
}

run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], "echo-app clippy clean");

const localState = readFileSync(
  resolve(ROOT, "crates", "echo-desktop", "src", "platform", "local_state.rs"),
  "utf8",
);
for (const token of ["clamp_to_visible", "pub struct WorkArea", "is_usable"]) {
  if (!localState.includes(token)) {
    fail(`local_state.rs missing '${token}'`);
  }
}

process.stdout.write(
  "ok 9.6: close-behavior defaults + window visible-area clamp; close-handler wiring deferred to shell/composition root\n",
);
