#!/usr/bin/env node
import { spawnSync } from "node:child_process";

const DESKTOP = "apps/desktop";
const PNPM = "pnpm";
const USE_SHELL = process.platform === "win32";

const recipes = {
  dev: {
    desc: "生成 IPC → 构建前端 → tauri dev",
    steps: [
      ["--dir", DESKTOP, "generate:ipc"],
      ["--dir", DESKTOP, "build"],
      ["--dir", DESKTOP, "tauri", "dev"],
    ],
  },
  release: {
    desc: "生成 IPC → 构建前端 → tauri build",
    steps: [
      ["--dir", DESKTOP, "generate:ipc"],
      ["--dir", DESKTOP, "build"],
      ["--dir", DESKTOP, "tauri", "build"],
    ],
  },
  build: {
    desc: "生成 IPC → 构建前端(不启动)",
    steps: [
      ["--dir", DESKTOP, "generate:ipc"],
      ["--dir", DESKTOP, "build"],
    ],
  },
};

const [, , subcommand = "help", ...passthrough] = process.argv;

function usage() {
  const lines = ["用法: pnpm echo <命令>", "", "命令:"];
  for (const [name, { desc }] of Object.entries(recipes)) {
    lines.push(`  ${name.padEnd(9)} ${desc}`);
  }
  console.log(lines.join("\n"));
}

if (subcommand === "help" || subcommand === "-h" || subcommand === "--help") {
  usage();
  process.exit(0);
}

const recipe = recipes[subcommand];
if (!recipe) {
  console.error(`未知命令: ${subcommand}\n`);
  usage();
  process.exit(1);
}

recipe.steps.forEach((step, index) => {
  const isLast = index === recipe.steps.length - 1;
  const args = [...step, ...(isLast ? passthrough : [])];
  const result = spawnSync(PNPM, args, { stdio: "inherit", shell: USE_SHELL });
  if (result.error) {
    console.error(`执行失败: pnpm ${args.join(" ")} — ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
});

// Tauri copies the DMG background correctly, but Finder's AppleScript styling
// can persist a stale volume-relative background reference on macOS. Re-write
// the final DMG metadata against its final mounted path before handing it off,
// and normalize the icon layout (.DS_Store 直接改字节) so that "Finder 忽略了
// 图标坐标" 这类问题不会再从 release 产物里溜出去。
if (subcommand === "release" && process.platform === "darwin") {
  const result = spawnSync(process.execPath, ["scripts/release/style-dmg.mjs"], {
    stdio: "inherit",
    shell: USE_SHELL,
  });
  if (result.error) {
    console.error(`DMG 收尾失败: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
}
