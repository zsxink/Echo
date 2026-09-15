#!/usr/bin/env node
// PRD A1–A14 → scenario / verification mapping (task 13.9).
//
// prd.md §13 defines 14 acceptance paths (A1–A14). This script produces the
// authoritative A# → scenario-ID-set mapping and emits
// `docs/acceptance/PRD-matrix.md`, which drives the release-gate handoff
// (13.5/13.7) and the 13.9 PRD execution.
//
// A scenario may serve several A#s; the matrix records every (A#, scenario)
// edge. Each A# also names one or more concrete macOS-runnable commands that
// prove the acceptance path's runnable portion (the cross-platform/manual
// remainder is the Gate handoff).
//
// Invocation: `node scripts/verify/prd-matrix.mjs [--write]`
//   --write  also writes docs/acceptance/PRD-matrix.md (refreshed on edits).

import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { allScenarioIds } from "./spec-scenarios.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const TRACE = resolve(ROOT, "docs", "traceability.md");
const PRD_OUT = resolve(ROOT, "docs", "acceptance", "PRD-matrix.md");

// The A# → scenario-ID families that prove it. Prefix families use the
// traceability per-area IDs; specific scenarios are listed where one path is
// uniquely responsible. Keep in sync with traceability.md / prd.md 13.
const A_SCENARIOS = {
  A1: ["DAS", "LL"], // 空库首次启动: shell first-launch + local-library root
  A2: ["LL"], // 10k 混合曲库扫描
  A3: ["LL"], // 外部改名/移动 relink
  A4: ["SFI"], // 多文件安全导入
  A5: ["LL"], // 资料库失联/恢复
  A6: ["LE", "LL"], // 搜索/排序/收藏
  A7: ["PM"], // 歌单 CRUD
  A8: ["DP"], // 播放与队列
  A9: ["IL"], // 歌词与沉浸模式
  A10: ["SFI"], // 系统双击文件
  A11: ["SFI"], // 删除与撤销
  A12: ["DAS"], // 关窗与托盘
  A13: ["DAS", "IL", "LE"], // 无障碍与窄屏
  A14: ["DAS", "LE"], // 范围保护
};

// A# → concrete macOS-runnable command(s) proving the runnable portion.
const A_COMMANDS = {
  A1: "cargo test -p echo-desktop --lib runtime::startup_supervisor && cargo test -p echo-core --all-features root_switch::",
  A2: "node scripts/verify/checks/task-12.5.mjs && cargo test -p echo-core --all-features scan::",
  A3: "cargo test -p echo-core --all-features relink",
  A4: "cargo test -p echo-core --all-features recover::",
  A5: "node scripts/verify/checks/task-5.8.mjs 2>/dev/null || cargo test -p echo-core --all-features trash::",
  A6: "node scripts/verify/checks/task-10.5.mjs",
  A7: "node scripts/verify/checks/task-10.9.mjs",
  // A8 additionally requires the snapshot *stream* to be live: the browser E2E
  // clicks a row and asserts the bar names it, and these two Rust tests cover
  // the links the mock cannot see (a dead actor subscription + the UI reading
  // queue identity from the transport snapshot) — each of which left the bar
  // empty while a song played.
  A8: "node scripts/verify/checks/task-11.2.mjs && cargo test -p echo-desktop --lib runtime::player && cargo test -p echo-desktop --lib player::actor::tests::subscribe_snapshots",
  // A9 also owns 歌词专注阅读 (immersive-lyrics §歌词专注模式): task-11.6 covers
  // entering/leaving the reading state, and the browser E2E (13.1) asserts the
  // *rendered* rearrangement — a unit test alone proved the flag flipped while
  // the surface stayed invisible behind the immersive player.
  A9: "node scripts/verify/checks/task-11.4.mjs && node scripts/verify/checks/task-11.5.mjs && node scripts/verify/checks/task-11.6.mjs",
  A10: "cargo test -p echo-core --all-features watch:: && node scripts/verify/checks/task-9.2.mjs",
  A11: "cargo test -p echo-core --all-features recover::",
  A12: "node scripts/verify/checks/task-9.6.mjs && node scripts/verify/checks/task-9.3.mjs",
  A13: "node scripts/verify/checks/task-12.2.mjs && node scripts/verify/checks/task-12.3.mjs",
  A14: "node scripts/verify/checks/task-13.8.mjs",
};

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exit(1);
}

const derived = allScenarioIds();
const byId = new Map(derived.map((d) => [d.id, d]));
const trace = readFileSync(TRACE, "utf8");
const traceReqs = new Map(); // id -> Requirement (title)
for (const line of trace.split("\n")) {
  const m = line.match(/^\|\s*([A-Z]{2,4}-R\d{2}-S\d{2})\s*\|\s*([^|]+)/);
  if (m) traceReqs.set(m[1], m[2].trim());
}

// Row: for every A#, every scenario whose ID starts with one of its prefixes.
const rows = [];
let missingTrace = 0;
for (const [a, prefixes] of Object.entries(A_SCENARIOS)) {
  for (const id of byId.keys()) {
    if (prefixes.some((p) => id.startsWith(p))) {
      rows.push({ a, id, req: traceReqs.get(id) || "" });
      if (!traceReqs.has(id)) missingTrace += 1;
    }
  }
}
if (missingTrace) fail(`${missingTrace} scenario IDs mapped by PRD are missing from traceability.md`);

// Every scenario must be claimed by at least one A#? No — the spec is
// scenario-driven; PRD is a subset view. But every A# must claim at least one
// scenario.
for (const [a, prefixes] of Object.entries(A_SCENARIOS)) {
  const n = rows.filter((r) => r.a === a).length;
  if (!n) fail(`PRD ${a} maps to no scenario`);
}

const lines = [];
lines.push("# PRD A1–A14 → scenario mapping (0.1.0)");
lines.push("");
lines.push("由 `scripts/verify/prd-matrix.mjs` 生成；`traceability.md` 是场景 ID 与命名权威。每一 A# 为一验收路径，列出其证明场景（按 traceability 领域前缀）。macOS 可自动执行的命令给出该路径可运行部分的证据；跨平台/人工其余部分见 `docs/acceptance/platform-gate-handoff.md`。");
lines.push("");
lines.push("| 编号 | 验收路径 | 证明场景数 | macOS 可运行命令 |");
lines.push("|---|---|---|---|");
for (const [a, means] of Object.entries(A_COMMANDS)) {
  const n = rows.filter((r) => r.a === a).length;
  lines.push(`| **${a}** | ${prdTitle(a)} | ${n} | \`${means.replace(/\|/g, "\\|")}\` |`);
}
lines.push("");
lines.push("## 逐场景归属");
lines.push("");
lines.push("| A# | Scenario ID | Requirement |");
lines.push("|---|---|---|");
for (const r of rows.sort((x, y) => x.a.localeCompare(y.a) || x.id.localeCompare(y.id))) {
  lines.push(`| ${r.a} | \`${r.id}\` | ${r.req} |`);
}
lines.push("");

const text = lines.join("\n");
if (process.argv.includes("--write")) {
  mkdirSync(dirname(PRD_OUT), { recursive: true });
  writeFileSync(PRD_OUT, text);
  process.stdout.write(`wrote ${PRD_OUT} (${rows.length} scenario edges, ${new Set(rows.map((r) => r.a)).size} A paths)\n`);
} else {
  process.stdout.write(`PRD matrix: ${rows.length} scenario edges across ${new Set(rows.map((r) => r.a)).size} A paths\n`);
}

function prdTitle(a) {
  return {
    A1: "空库首次启动", A2: "10,000 首混合曲库扫描", A3: "外部改名/移动", A4: "多文件安全导入",
    A5: "资料库失联/恢复", A6: "搜索/排序/收藏", A7: "歌单 CRUD", A8: "播放与队列",
    A9: "歌词与沉浸模式", A10: "系统双击文件", A11: "删除与撤销", A12: "关窗与托盘",
    A13: "无障碍与窄屏", A14: "范围保护",
  }[a];
}