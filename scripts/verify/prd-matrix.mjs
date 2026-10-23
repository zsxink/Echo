#!/usr/bin/env node
// PRD A1–A14 → scenario mapping consistency check (task 13.9).
//
// prd.md §13 defines 14 acceptance paths (A1–A14). `docs/acceptance/PRD-matrix.md`
// is a hand-maintained committed document — nothing regenerates it (the former
// generator's `--write` dirtied the worktree on every gate run while the gate
// asserted nothing). This script validates the committed matrix against the
// live scenario set so drift fails the gate with an actionable diff instead
// of silently misinforming the release handoff:
//   - every A# has a summary row naming a macOS-runnable command;
//   - the summary 证明场景数 equals the per-A# ownership rows;
//   - the ownership rows are exactly the live spec scenarios whose ID starts
//     with one of the path's traceability area prefixes (the PRD is a subset
//     view: scenarios in areas outside prd.md §13 stay unclaimed).
//
// Invocation: `node scripts/verify/prd-matrix.mjs` (exit 1 on drift).

import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { allScenarioIds } from "./spec-scenarios.mjs";

const ROOT = resolve(dirname(fileURLToPath(new URL(".", import.meta.url))), "..");
const PRD = resolve(ROOT, "docs", "acceptance", "PRD-matrix.md");

// A# → traceability area prefixes whose scenarios prove the acceptance path
// (prd.md §13). Keep in sync with traceability.md / prd.md 13; when a path's
// area ownership changes, update this table and the matrix in the same change.
const A_FAMILIES = {
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

function fail(msg) {
  process.stderr.write(`error: ${msg}\n`);
  process.exit(1);
}

if (process.argv.includes("--write")) {
  fail(`--write was removed: ${PRD} is hand-maintained; edit it directly and commit`);
}

const prd = readFileSync(PRD, "utf8");
const problems = [];

// Summary rows look like `| **A1** | 空库首次启动 | 106 | \`cargo test ...\` |`;
// command cells may contain escaped pipes (`\|\|`), so allow `\\.` escapes.
const summary = new Map();
const summaryRe = /^\|\s*\*\*(A\d{1,2})\*\*\s*\|([^|]*)\|\s*(\d+)\s*\|\s*((?:\\.|[^|])*)\|\s*$/gm;
for (const [, a, , count, cmd] of prd.matchAll(summaryRe)) {
  if (summary.has(a)) problems.push(`${a}: 摘要表存在重复行`);
  summary.set(a, { count: Number(count), cmd: cmd.trim() });
}

// Ownership rows live below `## 逐场景归属`: `| A1 | \`DAS-R01-S01\` | req |`.
const sectionAt = prd.indexOf("## 逐场景归属");
if (sectionAt < 0) {
  problems.push('缺少 "## 逐场景归属" 章节');
}
const owned = new Map();
if (sectionAt >= 0) {
  const ownershipRe = /^\|\s*(A\d{1,2})\s*\|\s*`([A-Z]{2,4}-R\d{2}-S\d{2})`\s*\|/gm;
  for (const [, a, id] of prd.slice(sectionAt).matchAll(ownershipRe)) {
    if (!owned.has(a)) owned.set(a, new Set());
    if (owned.get(a).has(id)) problems.push(`${a} 重复列出 \`${id}\``);
    owned.get(a).add(id);
  }
}

const live = allScenarioIds();
const expected = new Map();
for (const [a, prefixes] of Object.entries(A_FAMILIES)) {
  expected.set(a, live.filter((d) => prefixes.some((p) => d.id.startsWith(p))).map((d) => d.id));
}

for (const [a, ids] of expected) {
  const got = owned.get(a) || new Set();
  const missing = ids.filter((id) => !got.has(id));
  const stale = [...got].filter((id) => !ids.includes(id)).sort();
  if (missing.length) problems.push(`${a} 缺少场景行: ${missing.join(", ")}`);
  if (stale.length) problems.push(`${a} 存在规格中已不存在的场景行: ${stale.join(", ")}`);
  const row = summary.get(a);
  if (!row) {
    problems.push(`${a}: 摘要表缺少 \`| **${a}** | … | n | 命令 |\` 行`);
  } else {
    if (!/^`(.+)`$/.test(row.cmd)) problems.push(`${a}: 摘要表缺少 macOS 可运行命令`);
    if (row.count !== got.size) {
      problems.push(`${a}: 摘要"证明场景数"为 ${row.count},实际归属 ${got.size} 行`);
    }
  }
}
for (const a of summary.keys()) {
  if (!expected.has(a)) problems.push(`摘要表存在未知路径 ${a}(A_FAMILIES 未定义)`);
}
// Not every scenario must be claimed: the spec is scenario-driven and the PRD
// is a subset view (areas beyond prd.md §13 — CRP/PHA/PLL/SYN — are out of
// scope). Only PRD-covered area families are enforced bidirectionally above.

if (problems.length) {
  fail(
    `${PRD} 与当前场景集不一致(该表为手工维护,请更新后提交):\n` +
      problems.map((p) => `  - ${p}`).join("\n") +
      `\n  场景 ID 权威来源: docs/traceability.md;若验收路径的领域归属变化,请同步调整本脚本的 A_FAMILIES。`,
  );
}
const edges = [...expected.values()].reduce((n, ids) => n + ids.length, 0);
process.stdout.write(`prd-matrix: 14 条验收路径与 ${live.length} 个场景一致(${edges} 条归属边)\n`);
