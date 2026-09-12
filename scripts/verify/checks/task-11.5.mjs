#!/usr/bin/env node
// Task 11.5 check: plain-text lyrics, no-lyrics and source error/fallback states.
//
// Acceptance (immersive-lyrics spec):
//  - Pure text lyrics show as a scrollable list, no current-line indicator and
//    no automatic seek.
//  - No usable lyrics shows the "暂无歌词" empty state with song info, never
//    stale lyrics from a previous song.
//  - The lyrics source ("Echo 覆盖层" / "内嵌歌词" / "LRC 侧车文件") is labelled;
//    a corrupt higher-priority source already falls back to the next valid one
//    in Core (`select_effective_lyrics`, task 4.5).
// The ImmersivePlayer tests cover plain text, no-lyrics and source-label;
// `GetSongLyrics` passes the effective candidate (with fallback already done in
// Core). This check runs the same gates as 11.4 plus the specific assertions.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.5: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("cargo", ["test", "-p", "echo-core", "application::detail"], ROOT);
run("cargo", ["clippy", "-p", "echo-core", "-p", "echo-desktop", "-p", "echo-app", "--all-targets", "--all-features", "--", "-D", "warnings"], ROOT);
run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/ImmersivePlayer.test.tsx"],
  APP,
);

process.stdout.write("ok 11.5: plain-text / no-lyrics / source label + fallback verified\n");
