#!/usr/bin/env node
// Task 11.2 check: playback queue panel with blocked/error items.
//
// The UI snapshot now carries the full queue (`UiQueueEntry[]`: entryId,
// songId/title, isCurrent, failed) so the panel renders current + pending with
// failed entries surfaced. This check verifies:
//   - The Rust `runtime::player::map_snapshot` derives the queue (plus
//     per-round failed set) from the authoritative coordinator — runtime::player
//     tests, including a failed-entry case.
//   - The React `QueuePanel` renders from that snapshot (QueuePanel tests),
//     and "清空待播" only issues `clearPending` (never touches playlists).
//   - The app shell still typechecks and clippy is clean.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const PANEL = resolve(APP, "src/features/player/QueuePanel.tsx");
const PANEL_TEST = resolve(APP, "src/features/player/QueuePanel.test.tsx");
if (!existsSync(PANEL) || !existsSync(PANEL_TEST)) {
  process.stderr.write(`FAIL 11.2: missing QueuePanel.tsx or QueuePanel.test.tsx\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.2: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/QueuePanel.test.tsx"],
  APP,
);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);
run("cargo", ["test", "-p", "echo-desktop", "runtime::player::tests"], ROOT);

process.stdout.write("ok 11.2: queue panel (snapshot-driven, blocked/error + clear pending) verified\n");
