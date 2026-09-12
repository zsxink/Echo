#!/usr/bin/env node
// Task 11.3 check: immersive player overlay (vinyl cover, metadata, expand/
// collapse, track switching without exiting, wide/narrow + no-cover placeholder).
//
// The player bar's "展开播放器" opens an immersive overlay that reads the
// authoritative PlayerSnapshot and fetches `song_detail` for library metadata;
// collapsing only toggles UI state (never stops/resets playback); track
// switching updates content in place (stays mounted). This check verifies the
// component (ImmersivePlayer tests) plus typecheck and a clean echo-app clippy.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const COMPONENT = resolve(APP, "src/features/player/ImmersivePlayer.tsx");
const TEST = resolve(APP, "src/features/player/ImmersivePlayer.test.tsx");
if (!existsSync(COMPONENT) || !existsSync(TEST)) {
  process.stderr.write(`FAIL 11.3: missing ImmersivePlayer.tsx or ImmersivePlayer.test.tsx\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.3: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/ImmersivePlayer.test.tsx"],
  APP,
);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 11.3: immersive player (vinyl, metadata, expand/collapse, track switching) verified\n");
