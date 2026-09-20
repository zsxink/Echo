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
// This check gates on the PlayerBar tests (badge/import/no-fabrication) plus
// the snapshot mapping tests (temporary canImport) + echo-desktop lib tests +
// typecheck + echo-app clippy.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 11.7: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

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
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write(
  "ok 11.7: temporary item metadata (cover/duration) + import-to-library verified\n",
);
