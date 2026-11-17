#!/usr/bin/env node
// Task 11.7 check: temporary playback item identification + "import to library".
//
// Acceptance (desktop-playback §临时播放项边界 / safe-file-ingestion §源文件):
//  - A session-only temporary item (a file opened outside the active library)
//    is clearly identified in the UI (badge) and has no library SongId.
//  - An "导入到资料库" entry exists for the current temporary item and invokes
//    the desktop `import_current_temporary_file` command; the absolute path is
//    never sent to the WebView (the path stays in the coordinator/reader).
//  - Favorite / add-to-playlist / stats are disabled for temporary items; only
//    after an explicit import does the normal SongId appear (import → library
//    record, per Core's PlanImport).
//
// `--static-only` (release gate) keeps the wiring assertions and skips
// typecheck + cargo + Vitest: the Rust/TS behavior is covered by the
// `Rust (Linux workspace)` and `Frontend (Linux)` jobs.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");
const staticOnly = process.argv.includes("--static-only");

function fail(msg) {
  process.stderr.write(`FAIL 11.7: ${msg}\n`);
  process.exit(1);
}

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.7: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

// --- Static drift guard: the temporary-item wiring exists and the path never
//     crosses the IPC boundary. The behavior itself runs in the workspace and
//     frontend suites.
const bridge = readFileSync(resolve(APP, "src", "bridge", "index.ts"), "utf8");
if (!bridge.includes("import_current_temporary_file: EmptyArgs;")) {
  fail(
    "bridge no longer declares import_current_temporary_file as EmptyArgs: the absolute path must never be sent to the WebView",
  );
}
const commandsRs = readFileSync(
  resolve(ROOT, "apps", "desktop", "src-tauri", "src", "commands.rs"),
  "utf8",
);
const signatureStart = commandsRs.indexOf("pub fn import_current_temporary_file(");
if (signatureStart === -1) {
  fail("commands.rs no longer exposes import_current_temporary_file");
}
const signature = commandsRs.slice(signatureStart, commandsRs.indexOf(")", signatureStart));
if (/path\s*:/.test(signature)) {
  fail("import_current_temporary_file accepts a path argument: the desktop must read the current item itself");
}
if (!signature.includes("PlayerHandle")) {
  fail("import_current_temporary_file no longer reads the current item from the player handle");
}
const bootstrap = readFileSync(
  resolve(ROOT, "apps", "desktop", "src-tauri", "src", "bootstrap.rs"),
  "utf8",
);
if (!bootstrap.includes("commands::import_current_temporary_file")) {
  fail("bootstrap.rs no longer registers the import_current_temporary_file command");
}
const playerBar = readFileSync(resolve(APP, "src", "features", "player", "PlayerBar.tsx"), "utf8");
if (!playerBar.includes('bridge.call("import_current_temporary_file")')) {
  fail("PlayerBar.tsx no longer invokes import_current_temporary_file");
}
const playerBarTest = readFileSync(
  resolve(APP, "src", "features", "player", "PlayerBar.test.tsx"),
  "utf8",
);
for (const name of [
  "shows a small 导入 button only for a temporary item",
  "imports the current temporary file when clicked",
  "shows 导入中 and ignores repeated clicks until the request completes",
]) {
  if (!playerBarTest.includes(name)) {
    fail(`PlayerBar.test.tsx lost the temporary-item test '${name}'`);
  }
}

if (!staticOnly) {
  run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
  run(
    "pnpm",
    ["--filter", "@echo/desktop", "test", "--", "--run", "src/features/player/PlayerBar.test.tsx"],
    APP,
  );
  run("cargo", ["test", "-p", "echo-desktop", "platform::import"], ROOT);
  run("cargo", ["test", "-p", "echo-desktop", "runtime::player"], ROOT);
  // The temporary item's presentation metadata (title/artist/album/duration/
  // cover) is read straight off the opened file: duration is probe-owned, so
  // `read_temporary_metadata` has to probe the bytes itself or the queue row
  // reads 时长未知 while the engine plays the file with a known length.
  run(
    "cargo",
    ["test", "-p", "echo-desktop", "runtime::services::tests::temporary_metadata"],
    ROOT,
  );
  process.stdout.write(
    "ok 11.7: temporary item metadata (cover/duration) + import-to-library verified\n",
  );
} else {
  process.stdout.write(
    "ok 11.7: temporary-item wiring intact (static-only; behavior delegated to workspace-test/frontend-test)\n",
  );
}
