#!/usr/bin/env node
// Task 10.5 check: 曲库视图（全部 / 最近 100 / 喜欢 / 歌单）、搜索、四排序双
// 方向、服务端 cursor、清空搜索与请求取消。
//
// Fails unless the library view suites pass: SongList (search/sort/cursor),
// SongRow, 最近播放导航计数, favoriteSync, PlaylistsView.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function fail(step, msg) {
  process.stderr.write(`FAIL 10.5: ${step}: ${msg}\n`);
  process.exit(1);
}

for (const filter of [
  "src/features/library/SongList.test.tsx",
  "src/features/library/SongRow.test.tsx",
  "src/features/library/libraryNavCounts.test.tsx",
  "src/features/library/favoriteSync.test.tsx",
  "src/features/playlists/PlaylistsView.test.tsx",
]) {
  const r = spawnSync("pnpm", ["--filter", "@echo/desktop", "test", filter], {
    cwd: APP,
    encoding: "utf8",
  });
  const out = `${r.stdout || ""}${r.stderr || ""}`;
  if (r.status !== 0) fail("vitest", `${filter}\n${out}`);
  const files = Number((out.match(/Test Files\s+(\d+)/) ?? [])[1] ?? 0);
  if (files !== 1) fail("vitest", `'${filter}' matched ${files} test files (expected exactly 1)`);
}

process.stdout.write("ok 10.5: library views (all / recent / liked / playlists) + search + sort + cursor hold\n");
