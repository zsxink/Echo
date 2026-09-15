#!/usr/bin/env node
// Task check for the "wire-desktop-system-dialogs" change: the shell must be
// wired to real OS dialogs/reveal (not a cancelling test double), the WebView
// capability set must stay free of dialog/fs privileges, and the initialize /
// status page must claim the workspace grid area.
//
// Asserts, all local and offline:
//   - apps/desktop/src-tauri/Cargo.toml declares tauri-plugin-dialog and
//     tauri-plugin-opener.
//   - apps/desktop/src-tauri/src/main.rs wires AppServices::with_runtime with a
//     TauriDialogs adapter (not AppServices::new's cancelling default).
//   - capabilities/main.json grants no dialog:/fs:/shell: permission (the
//     minimal-viable posture from task 7.7 is preserved).
//   - .workspace-empty declares grid-area: workspace so the unconfigured /
//     status view fills the workspace column.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const CARGO = resolve(ROOT, "apps", "desktop", "src-tauri", "Cargo.toml");
const MAIN = resolve(ROOT, "apps", "desktop", "src-tauri", "src", "main.rs");
const DIALOGS = resolve(ROOT, "apps", "desktop", "src-tauri", "src", "dialogs.rs");
const CAPABILITY = resolve(ROOT, "apps", "desktop", "src-tauri", "capabilities", "main.json");
// `.workspace-empty` is an implementation-only surface (the prototype always
// ships a populated library), so it lives in the app's own stylesheet layer.
const WORKSPACE_CSS = resolve(ROOT, "apps", "desktop", "src", "styles", "app-extras.css");

function fail(msg) {
  process.stderr.write(`FAIL wire-desktop-system-dialogs: ${msg}\n`);
  process.exit(1);
}
function ok(msg) {
  process.stdout.write(`ok wire-desktop-system-dialogs: ${msg}\n`);
}

// 1. Plugins declared.
const cargo = readFileSync(CARGO, "utf8");
if (!cargo.includes("tauri-plugin-dialog")) fail("tauri-plugin-dialog missing from Cargo.toml");
if (!cargo.includes("tauri-plugin-opener")) fail("tauri-plugin-opener missing from Cargo.toml");
ok("Cargo.toml declares tauri-plugin-dialog + tauri-plugin-opener");

// 2. Composition root wired to the real adapter.
const main = readFileSync(MAIN, "utf8");
if (!main.includes("TauriDialogs::new")) fail("main.rs does not construct TauriDialogs");
if (!main.includes("AppServices::with_runtime")) {
  fail("main.rs does not wire AppServices::with_runtime (cancelling test default?)");
}
const dialogsSrc = readFileSync(DIALOGS, "utf8");
if (!dialogsSrc.includes("impl SystemDialogs for TauriDialogs")) {
  fail("dialogs.rs does not implement SystemDialogs for TauriDialogs");
}
if (!dialogsSrc.includes("blocking_pick_folder")) fail("TauriDialogs::pick_library_directory lacks a native folder pick");
if (!dialogsSrc.includes("blocking_pick_files")) fail("TauriDialogs::pick_audio_files lacks a native file pick");
if (!dialogsSrc.includes("reveal_item_in_dir")) fail("TauriDialogs::reveal does not use reveal_item_in_dir");
ok("main.rs wires AppServices::with_runtime over TauriDialogs; dialogs.rs uses native pickers + reveal");

// 3. Capability posture unchanged: no dialog/fs/shell privilege granted.
const capability = JSON.parse(readFileSync(CAPABILITY, "utf8"));
const perms = capability.permissions || [];
const forbidden = perms.filter((p) =>
  /(^|:)dialog:|(^|:)fs:|(^|:)shell:|(^|:)opener:|(^|:)process:|(^|:)sql:/.test(p),
);
if (forbidden.length > 0) {
  fail(`capability grants privileged permission(s): ${forbidden.join(", ")}`);
}
ok(`capabilities/main.json stays minimal (no dialog/fs/shell/opener/process/sql): ${perms.length} permission(s)`);

// 4. Initialize / status page claims the workspace area.
const css = readFileSync(WORKSPACE_CSS, "utf8");
const block = css.slice(css.indexOf(".workspace-empty {"), css.indexOf(".workspace-empty {") + 600);
if (!block.includes("grid-area: workspace")) {
  fail(".workspace-empty does not declare grid-area: workspace");
}
ok(".workspace-empty declares grid-area: workspace (initialize/status fills the workspace column)");

process.stdout.write("ok 3.1: wire-desktop-system-dialogs checks pass\n");