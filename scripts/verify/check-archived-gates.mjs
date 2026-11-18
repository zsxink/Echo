#!/usr/bin/env node
// 归档门禁残留检查：证明归档 task 的脚本资产与实际执行引用一致。
// 判据与例外见 openspec/changes/trim-archived-gates-and-local-artifacts/design.md D1/D2/D3。

import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const MANIFEST = resolve(ROOT, 'scripts/verify/manifest.json');
const CHECKS = resolve(ROOT, 'scripts/verify/checks');
const CI = resolve(ROOT, '.github/workflows/ci.yml');
const GUARD_FILE = 'scripts/verify/injection-governance.mjs';

// D1：只有这些入口构成"实际执行引用"。manifest 自身、checks 内部与历史文档不算。
const ENTRY_FILES = [
  '.github/workflows/ci.yml',
  'scripts/verify/run-task.mjs',
  'scripts/verify/injection-governance.mjs',
  'scripts/verify/ci-governance.mjs',
  'scripts/verify/self-test.mjs',
  'scripts/verify/validate-scenario-commands.mjs',
  'scripts/verify/check-scenario-evidence.mjs',
  'package.json',
  'scripts/echo.mjs',
];

// 本次归档清理的权威清单（删除前 = 待删集合；删除后 = 应全部消失）。
const EXPECTED_REMOVABLE = [
  'task-1.4.mjs', 'task-1.8.mjs',
  'task-2.2.mjs', 'task-2.3.mjs', 'task-2.4.mjs', 'task-2.5.mjs',
  'task-2.6.mjs', 'task-2.7.mjs', 'task-2.8.mjs',
  'task-8.1.mjs', 'task-8.2.mjs', 'task-8.3.mjs', 'task-8.4.mjs',
  'task-8.5.mjs', 'task-8.6.mjs', 'task-8.7.mjs', 'task-8.8.mjs',
  'task-8.9.mjs', 'task-8.10.mjs', 'task-8.11.mjs',
  'task-10.1.mjs', 'task-10.2.mjs', 'task-10.3.mjs', 'task-10.4.mjs',
  'task-10.5.mjs', 'task-10.6.mjs', 'task-10.7.mjs', 'task-10.8.mjs',
  'task-10.9.mjs', 'task-10.10.mjs',
  'task-11.1.mjs', 'task-11.2.mjs', 'task-11.3.mjs',
  'task-11.4.mjs', 'task-11.5.mjs', 'task-11.6.mjs',
  'task-12.1.mjs', 'task-12.2.mjs', 'task-12.3.mjs',
  'task-13.6.mjs', 'task-16.2.mjs',
];
const KEPT_WITH_CONSUMER = 'task-13.2.mjs';

const problems = [];
function require(condition, message) {
  if (!condition) problems.push(message);
}

function scriptsOf(tasks) {
  const result = new Set();
  for (const task of tasks) {
    for (const command of task.commands ?? []) {
      for (const match of String(command.cmd ?? '').matchAll(/scripts\/verify\/checks\/([\w.-]+\.mjs)/g)) {
        result.add(match[1]);
      }
    }
  }
  return result;
}

let manifest = null;
try {
  manifest = JSON.parse(readFileSync(MANIFEST, 'utf8'));
} catch (error) {
  problems.push(`无法读取 manifest.json: ${error.message}`);
}

let corpus = '';
for (const relative of ENTRY_FILES) {
  const absolute = resolve(ROOT, relative);
  if (!existsSync(absolute)) {
    problems.push(`执行入口缺失: ${relative}`);
    continue;
  }
  corpus += `${readFileSync(absolute, 'utf8')}\n`;
}

let phase = 'unknown';
let archivedCount = 0;
let archivedOnlyCount = 0;
let removable = [];
let kept = [];

if (manifest !== null) {
  const archived = manifest.tasks.filter((task) => task.archived === true);
  const active = manifest.tasks.filter((task) => task.archived !== true);
  archivedCount = archived.length;

  const activeScripts = scriptsOf(active);
  const archivedScripts = scriptsOf(archived);
  const archivedOnly = [...archivedScripts].filter((name) => !activeScripts.has(name));
  archivedOnlyCount = archivedOnly.length;

  const isReferenced = (name) => corpus.includes(name);
  removable = archivedOnly.filter((name) => !isReferenced(name)).sort();
  kept = archivedOnly.filter((name) => isReferenced(name)).sort();

  const present = EXPECTED_REMOVABLE.filter((name) => existsSync(resolve(CHECKS, name)));
  const absent = EXPECTED_REMOVABLE.filter((name) => !existsSync(resolve(CHECKS, name)));
  if (present.length === EXPECTED_REMOVABLE.length) {
    phase = 'before';
  } else if (absent.length === EXPECTED_REMOVABLE.length) {
    phase = 'after';
  } else {
    problems.push(`清理处于中间状态：${present.length} 个待删脚本仍在、${absent.length} 个已删`);
  }

  if (phase === 'before') {
    require(
      JSON.stringify(removable) === JSON.stringify([...EXPECTED_REMOVABLE].sort()),
      `待删集合与权威清单不一致：实际为 ${removable.join(' ') || '(空)'}`,
    );
    require(
      kept.length === 1 && kept[0] === KEPT_WITH_CONSUMER,
      `保留例外不符：${kept.join(' ') || '(空)'}`,
    );
  } else if (phase === 'after') {
    require(removable.length === 0, `清单仍登记无执行引用的脚本：${removable.join(' ')}`);
  }

  // D2：例外存在，且注入治理证明仍把它作为 guard 目标。
  require(existsSync(resolve(CHECKS, KEPT_WITH_CONSUMER)), `${KEPT_WITH_CONSUMER} 缺失`);
  require(isReferenced(KEPT_WITH_CONSUMER), `${KEPT_WITH_CONSUMER} 失去执行消费者`);
  const guardText = readFileSync(resolve(ROOT, GUARD_FILE), 'utf8');
  require(
    guardText.includes(KEPT_WITH_CONSUMER),
    `${GUARD_FILE} 不再指向 ${KEPT_WITH_CONSUMER}`,
  );

  // D3：manifest 与 ci.yml 中登记的门禁路径必须真实存在。
  const registered = new Set([...archivedScripts, ...activeScripts]);
  const ciText = readFileSync(CI, 'utf8');
  for (const match of ciText.matchAll(/scripts\/verify\/checks\/([\w.-]+\.mjs)/g)) {
    registered.add(match[1]);
  }
  for (const name of registered) {
    require(
      existsSync(resolve(CHECKS, name)),
      `悬空引用（manifest/CI）: scripts/verify/checks/${name}`,
    );
  }
}

const lines = [
  `check-archived-gates: phase=${phase} archived=${archivedCount} archived-only=${archivedOnlyCount} removable=${removable.length} kept-with-consumer=${kept.length}`,
  ...removable.map((name) => `  removable:   scripts/verify/checks/${name}`),
  ...kept.map((name) => `  kept:        scripts/verify/checks/${name} (consumer: ${GUARD_FILE})`),
];

if (problems.length > 0) {
  process.stderr.write(`${lines.join('\n')}\n`);
  for (const problem of problems) process.stderr.write(`FAIL check-archived-gates: ${problem}\n`);
  process.exit(1);
}

process.stdout.write(`${lines.join('\n')}\nPASS check-archived-gates\n`);
