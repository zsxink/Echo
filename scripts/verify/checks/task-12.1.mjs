#!/usr/bin/env node
// Task 12.1 check: single Overlay Manager.
//
// Acceptance (desktop-app-shell spec §键盘焦点与 Escape 行为必须可访问):
//  - A single floating-layer stack closes exactly one layer (the topmost
//    priority) per Escape, in the order 阻断确认 → 选择器 → 菜单/队列 → 歌词专注 →
//    沉浸 → 窄屏侧边栏 — never several at once.
//  - Focus enters an overlay on open and is restored to the trigger on close.
//  - Tab is trapped inside the open overlay; a role="menu" uses roving focus
//    (Up/Down/Home/End move the active item, Enter activates).
// This check gates on overlays.test.tsx (the shared stack + focus + roving
// menu), plus typecheck + echo-app clippy.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.1: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run", "src/app/overlays.test.tsx"], APP);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 12.1: single overlay stack (Escape priority, focus trap/restore, roving menu) verified\n");
