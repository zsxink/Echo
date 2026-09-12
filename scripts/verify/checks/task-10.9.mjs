#!/usr/bin/env node
// Task 10.9 check: playlist management UI — create/rename/delete (40-grapheme
// validation), add-to-playlist multi-select selector, remove, blocked members.
// Verifies the playlist view + selector component tests and a clean typecheck.
// Membership dedup/append order and "delete playlist never deletes files" are
// enforced by echo-core (tasks 6.6/6.7) and covered by their core tests.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const REQUIRED = [
  resolve(APP, "src/features/playlists/PlaylistsView.test.tsx"),
  resolve(APP, "src/features/playlists/AddToPlaylistDialog.test.tsx"),
];
for (const file of REQUIRED) {
  if (!existsSync(file)) {
    process.stderr.write(`FAIL 10.9: missing ${file}\n`);
    process.exit(1);
  }
}

const STEPS = [
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  [
    "playlist-tests",
    [
      "--filter",
      "@echo/desktop",
      "test",
      "--",
      "--run",
      "src/features/playlists/PlaylistsView.test.tsx",
      "src/features/playlists/AddToPlaylistDialog.test.tsx",
    ],
  ],
];

for (const [name, args] of STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 10.9: '${name}' exited ${r.status}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write(
  "ok 10.9: playlist create/rename/delete, selector, remove, blocked-member UI verified\n",
);
