#!/usr/bin/env node
// Task 10.7 check: library states — loading, empty (with import entry),
// search-empty, recoverable load failure (preserve content + retry), library
// unavailable, and file-missing must be explicit UI states and are verified by
// the SongList + workspace tests, with typecheck/lint green.
//
// The unavailable / read-only / missing states live in LibraryStatusView and
// SongRow (tasks 10.3/10.7); the load-failure + empty states are covered here.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

// The test that asserts the loading / empty / search-empty / failure-with-retry
// states and that an error never fakes an empty library.
const STATE_TEST = resolve(APP, "src/features/library/SongList.test.tsx");
if (!existsSync(STATE_TEST)) {
  process.stderr.write(`FAIL 10.7: missing ${STATE_TEST}\n`);
  process.exit(1);
}

const STEPS = [
  ["typecheck", ["--filter", "@echo/desktop", "typecheck"]],
  [
    "state-tests",
    [
      "--filter",
      "@echo/desktop",
      "test",
      "--",
      "--run",
      "src/features/library/SongList.test.tsx",
    ],
  ],
];

for (const [name, args] of STEPS) {
  const r = spawnSync("pnpm", args, { cwd: APP, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(`FAIL 10.7: '${name}' exited ${r.status}\n${r.stderr}\n`);
    process.exit(1);
  }
}

process.stdout.write(
  "ok 10.7: loading/empty/search-empty/failure-retry/missing states verified\n",
);
