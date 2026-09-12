#!/usr/bin/env node
// Task 10.8 check: read-only song detail + reveal + delete confirm/undo and
// forward-error states. Verifies (component tests) that the detail shows only
// a relative path, reveal/cancel/failed-delete never fake a success, and the
// 10-second undo affordance appears only on a real (accepted) delete.
//
// The delete_song / undo_delete / reveal_song IPC commands are service-layer
// (crates/echo-desktop/src/runtime/services.rs); their Tauri registration is
// part of the composition-root wiring, not this UI task.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const MENU_TEST = resolve(APP, "src/features/library/SongMenu.test.tsx");
if (!existsSync(MENU_TEST)) {
  process.stderr.write(`FAIL 10.8: missing ${MENU_TEST}\n`);
  process.exit(1);
}

const STEPS = [
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  ["menu-tests", ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/library/SongMenu.test.tsx"]],
];

for (const [name, args] of STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 10.8: '${name}' exited ${r.status}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write("ok 10.8: relative-path detail, reveal, delete confirm/undo/error states verified\n");
