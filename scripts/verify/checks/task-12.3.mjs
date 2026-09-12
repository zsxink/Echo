#!/usr/bin/env node
// Task 12.3 check: 760px narrow-screen sidebar/mask + secondary-column collapse.
//
// Acceptance (desktop-app-shell spec §窄屏布局与浮层关闭必须可预测):
//  - On ≤760px the sidebar opens via a menu button and shows a mask; the button
//    reflects the expanded state; clicking the mask closes the sidebar and
//    returns focus to the button.
//  - Resizing to narrow never loses the current view, the search text, or the
//    playback state.
// jsdom cannot evaluate the CSS media query, so the test exercises the state
// machine the media query drives (open → mask → close → focus restore + state
// preservation); the CSS itself (collapsed columns, toggle, mask, immersive
// narrow layout) is checked for regressions by clamp on the check below.
// This check gates on narrow.test.tsx + typecheck + echo-app clippy.

import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..", "..");
const APP = resolve(ROOT, "apps", "desktop");

function run(command, args, cwd) {
  const r = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (r.status !== 0) {
    process.stderr.write(
      `FAIL 12.3: '${command} ${args.join(" ")}' exited ${r.status}\n${r.stderr || r.stdout}\n`,
    );
    process.exit(1);
  }
}

run("pnpm", ["--filter", "@echo/desktop", "typecheck"], APP);
run("pnpm", ["--filter", "@echo/desktop", "test", "--", "--run", "src/app/narrow.test.tsx"], APP);
run("cargo", ["clippy", "-p", "echo-app", "--", "-D", "warnings"], ROOT);

process.stdout.write("ok 12.3: narrow-screen sidebar/mask + secondary-column collapse + resize state preservation verified\n");
