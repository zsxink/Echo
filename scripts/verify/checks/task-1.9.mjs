#!/usr/bin/env node
// macOS Tauri packaging Gate. It builds the arm64 .app, checks that the supported
// audio associations and arm64 libmpv framework are bundled, then proves
// cold/hot single-instance delivery and explicit exit using only Gate-only
// environment variables (no frontend privilege is added).

import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { inspectMacApp } from "./inspect-macos-app.mjs";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const BUNDLE = resolve(ROOT, "target", "aarch64-apple-darwin", "release", "bundle", "macos", "Echo.app");

function fail(message) {
  process.stderr.write(`FAIL 1.9: ${message}\n`);
  process.exit(1);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: "utf8", ...options });
  if (result.status !== 0) {
    fail(`${command} ${args.join(" ")} exited ${result.status}\n${result.stdout || ""}${result.stderr || ""}`);
  }
  return `${result.stdout || ""}${result.stderr || ""}`;
}

if (process.platform !== "darwin") {
  fail("the current Stage 1 Gate is intentionally macOS-local; run it on macOS");
}

run("rustup", ["target", "add", "aarch64-apple-darwin"]);
run("pnpm", ["--filter", "@echo/desktop", "tauri", "build", "--bundles", "app", "--target", "aarch64-apple-darwin"]);
run("codesign", ["--force", "--deep", "--sign", "-", BUNDLE]);

try {
  await inspectMacApp(BUNDLE);
} catch (error) {
  fail(error.message);
}

process.stdout.write("ok 1.9: macOS Tauri app bundles libmpv, file associations, tray and single-instance Gate pass\n");
