#!/usr/bin/env node
// Task 11.4 check: synced lyrics — current line, seek positioning, click-to-seek,
// out-of-order/out-of-range handling, UI progress interpolation.
//
// The effective lyrics are served by the Core `GetSongLyrics` use case (source
// selection reuses the domain `select_effective_lyrics`; out-of-order/out-of-range
// timestamps are pre-sorted/filtered by the LRC parser in task 4.5) and exposed
// via the `get_lyrics` command (IPC DTO drift-locked by the generator). The
// ImmersivePlayer renders the current line from the authoritative snapshot
// position and sends `seek` on click. This check verifies:
//   - echo-core `application::detail` (GetSongLyrics timed-lines + source)
//   - echo-desktop `runtime::player` (snapshot map, unaffected)
//   - echo-core + echo-desktop + echo-app clippy
//   - ImmersivePlayer tests (current-line highlight + click-to-seek)
//   - frontend typecheck + IPC generator drift

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.4: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("cargo", ["test", "-p", "echo-core", "application::detail"], ROOT);
run("cargo", ["test", "-p", "echo-desktop", "runtime::player::tests"], ROOT);
run("cargo", ["test", "-p", "echo-desktop", "ipc::generate::tests"], ROOT);
run("cargo", ["clippy", "-p", "echo-core", "-p", "echo-desktop", "-p", "echo-app", "--all-targets", "--all-features", "--", "-D", "warnings"], ROOT);
run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/ImmersivePlayer.test.tsx"],
  APP,
);

process.stdout.write("ok 11.4: synced lyrics (current line, click-to-seek, out-of-order, interpolation) verified\n");
