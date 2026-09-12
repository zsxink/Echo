#!/usr/bin/env node
// Task 11.1 check: persistent player bar driven by the authoritative
// PlayerSnapshot.
//
// The React `PlayerBar` renders cover/info/favorite, transport, progress,
// volume/mute, mode, queue toggle and empty state — every value from the
// snapshot (component already present). This check verifies the wiring slice:
//   - `playerStore` is fed by the `player://snapshot` event via
//     `startPlayerEvents()` (never fabricated) — playerStore tests.
//   - The Rust runtime maps the raw PlayerSnapshot + coordinator queue into the
//     UI shape (`runtime::player::map_snapshot`), including currentSongId /
//     currentTitle derivation — runtime::player tests.
//   - The live snapshot event is emitted by the root (echo-app compiles).

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

const STORE_TEST = resolve(APP, "src/player/playerStore.test.ts");
if (!existsSync(STORE_TEST)) {
  process.stderr.write(`FAIL 11.1: missing ${STORE_TEST}\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.1: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run", "src/player/playerStore.test.ts"], APP);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);
run("cargo", ["test", "-p", "echo-desktop", "runtime::player::tests"], ROOT);

process.stdout.write("ok 11.1: persistent player bar snapshot (PlayerSnapshot-driven) verified\n");
