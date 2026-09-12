#!/usr/bin/env node
// Task 12.2 check: full keyboard path and screen-reader names/states + axe.
//
// Acceptance (desktop-app-shell spec §键盘焦点与 Escape 行为必须可访问):
//  - Tab/Shift+Tab/Enter/Space/arrows/Home/End reach nav, search, import,
//    theme, settings, playback and queue controls with a visual focus order and
//    a readable name/state for every control.
//  - axe reports no blocking (critical/serious) accessibility violations.
// This check gates on accessibility.test.tsx (Testing Library keyboard path +
// axe scan of the configured shell), plus typecheck + echo-app clippy.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.2: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "lint"], APP);
run(
  "pnpm",
  ["--filter", "@echo/desktop", "test", "--", "--run", "src/app/accessibility.test.tsx"],
  APP,
);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 12.2: full keyboard path + screen-reader names/states + axe (no blocking violations) verified\n");
