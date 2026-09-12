#!/usr/bin/env node
// Task 9.3 check: productionized macOS menu-bar / Windows-Linux tray.
//
// The entry must offer a current-play summary plus 播放/暂停、上一首、下一首、
// 显示、退出 (desktop-app-shell spec), with the summary/label/command mapping
// as unit-tested decision logic in echo-desktop and the thin shell building
// the widgets + dispatching through a sink.
//
// Fails (nonzero) unless:
//   1. the status_menu unit tests pass (summary lines, play/pause label,
//      transport→command mapping, show/quit are shell lifecycle, sink seam);
//   2. the binary is clippy-clean;
//   3. main.rs builds the full six-item tray menu (summary header + transport
//      + show + quit) and dispatches transport clicks through the sink.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");

function fail(msg) {
  process.stderr.write(`FAIL 9.3: ${msg}\n`);
  process.exit(1);
}

function run(cmd, args, what) {
  const result = spawnSync(cmd, args, { cwd: ROOT, encoding: "utf8" });
  if (result.status !== 0) {
    fail(`${what}: ${cmd} ${args.join(" ")} exited ${result.status}`);
  }
  return result.stdout;
}

// 1. status_menu decision logic is green.
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "platform::status_menu"],
  "status_menu tests",
);
if (!testOut.includes("test result: ok.")) {
  fail("status_menu tests not green");
}

// 2. The thin binary stays clippy-clean.
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], "echo-app clippy clean");

// 3. The shell builds the full entry + dispatches through the sink.
const main = readFileSync(resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs"), "utf8");
for (const token of [
  "status_menu::MENU_PLAY_PAUSE",
  "status_menu::MENU_PREVIOUS",
  "status_menu::MENU_NEXT",
  "status_menu::MENU_SHOW",
  "status_menu::MENU_QUIT",
  "sink.on_command(command)",
  "NoopSink",
  "play_pause_label",
]) {
  if (!main.includes(token)) {
    fail(`main.rs missing '${token}'`);
  }
}

process.stdout.write("ok 9.3: productionized menu-bar/tray entry (summary + transport + show/quit)\n");
