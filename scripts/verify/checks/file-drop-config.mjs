#!/usr/bin/env node
// Static window-file-drop contract (issue #117).
//
// The three-platform *behaviour* is covered by the native smoke tests; this
// check is the executable local evidence for the parts that must never drift:
// both window configs enable Tauri's drag/drop event source, the WebView
// capability set stays minimal, the shell owns the capture, and the overlay
// stays non-interactive with a reduced-motion fallback.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const read = (relative) => readFileSync(resolve(ROOT, relative), "utf8");
const json = (relative) => JSON.parse(read(relative));
const fail = (message) => {
  process.stderr.write(`FAIL file-drop-config: ${message}\n`);
  process.exit(1);
};

const baseWindow = json("apps/desktop/src-tauri/tauri.conf.json").app.windows[0];
const windowsWindow = json("apps/desktop/src-tauri/tauri.windows.conf.json").app.windows[0];
if (baseWindow.dragDropEnabled !== true || windowsWindow.dragDropEnabled !== true) {
  fail("both window configs must explicitly set dragDropEnabled: true");
}
// Windows adds `decorations: false`; every other window field must be identical
// so a future platform edit cannot silently change only one drop path.
const withoutDecorations = ({ decorations, ...rest }) => JSON.stringify(rest);
if (withoutDecorations(baseWindow) !== withoutDecorations(windowsWindow)) {
  fail("base and Windows window definitions drifted beyond decorations");
}

const expectedPermissions = [
  "core:default",
  "core:window:default",
  "core:window:allow-start-dragging",
  "core:window:allow-minimize",
  "core:window:allow-toggle-maximize",
  "core:window:allow-close",
];
const permissions = json("apps/desktop/src-tauri/capabilities/main.json").permissions;
if (JSON.stringify(permissions) !== JSON.stringify(expectedPermissions)) {
  fail("the WebView capability set changed; file drop must not expand it");
}

const bootstrap = read("apps/desktop/src-tauri/src/bootstrap.rs");
if (!bootstrap.includes(".on_window_event(") || !bootstrap.includes("file_drop::handle_drag_drop")) {
  fail("the shell must own the native DragDrop event");
}
const gate = read("apps/desktop/src-tauri/src/file_open_gate.rs");
if (!gate.includes("deliver_file_opens") || !gate.includes("accepted_opens")) {
  fail("a drop must be delivered as one batch through the file-open gate");
}
const command = read("apps/desktop/src-tauri/src/commands_file_open.rs");
if (!command.includes("play_opened_files") || command.includes("play_temporary_file")) {
  fail("the retired per-file command must not survive");
}
const batchStart = command.indexOf("pub fn play_opened_files(");
const batchEnd = command.indexOf("\n#[tauri::command]", batchStart + 1);
const batchBody = command.slice(batchStart, batchEnd === -1 ? undefined : batchEnd);
const batchCode = batchBody
  .split("\n")
  .filter((line) => !line.trim().startsWith("//"))
  .join("\n");
if (batchStart === -1 || batchCode.includes("guard_writes")) {
  fail("temporary playback must not go through the write guard");
}

const shellCss = read("apps/desktop/src/styles/shell.css");
for (const declaration of [
  ".file-drop-overlay",
  "pointer-events: none",
  "prefers-reduced-motion: reduce",
]) {
  if (!shellCss.includes(declaration)) fail(`shell.css is missing ${declaration}`);
}
const app = read("apps/desktop/src/app/App.tsx");
if (!app.includes("<FileDropOverlay />") || !app.includes("play_opened_files")) {
  fail("the shell must mount the overlay and call the batch command");
}
const overlay = read("apps/desktop/src/app/FileDropOverlay.tsx");
if (overlay.includes("useOverlay(") || overlay.includes("tier:")) {
  fail("the drop hint must not register an OverlayTier");
}

process.stdout.write("ok file-drop-config: window configs, capability, shell capture and overlay agree\n");
