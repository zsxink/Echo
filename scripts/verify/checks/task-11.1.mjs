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
//     currentTitle derivation — runtime::player tests, required by name.
//   - The snapshot stream reaches the UI end to end (coordinator → port →
//     forwarder → UI snapshot) — the forwarder test, required by name. "The
//     root compiles" is not enough: a dead subscription emitted nothing and
//     still passed.

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

// The bar is only "PlayerSnapshot-authoritative" if the snapshot stream actually
// reaches it: the chain coordinator → port → forwarder → UI snapshot must be
// asserted end to end, and the UI must take queue identity from the coordinator
// (the actor cannot know a queue-entry id). Requiring these by name, rather than
// only "the module compiles and its tests run", is what keeps a dead
// subscription or a wrong identity source from passing as green.
const runtime = spawnSync("cargo", ["test", "-p", "echo-desktop", "--lib", "runtime::player::tests"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (runtime.status !== 0) {
  process.stderr.write(`FAIL 11.1: cargo test runtime::player::tests exited ${runtime.status}\n${runtime.stdout}\n${runtime.stderr}\n`);
  process.exit(1);
}
for (const name of [
  "forwarder_emits_a_ui_snapshot_that_names_the_playing_song",
  "ui_queue_identity_comes_from_the_coordinator_not_the_transport_snapshot",
]) {
  if (!runtime.stdout.includes(name)) {
    process.stderr.write(`FAIL 11.1: missing test ${name}\n`);
    process.exit(1);
  }
}

process.stdout.write("ok 11.1: persistent player bar snapshot (PlayerSnapshot-driven) verified\n");
