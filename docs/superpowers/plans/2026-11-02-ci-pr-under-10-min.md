# CI PR Under 10 Minutes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 PR CI 墙钟从 21m46s 压到 10 分钟以内，同时不减少任何检查、场景、证据或 task。

**Architecture:** 先改验证脚本的契约（分段、委托、`--all-targets`、分片 helper），再扩展 `task-1.5` 作为 CI 形状的结构门禁，最后按新契约重排 `.github/workflows/ci.yml` 的 job 图。所有 CI 结构改动都有对应的注入证明，确保结构断言不是恒真。

**Tech Stack:** GitHub Actions、Node.js 22 ESM 验证脚本、Rust workspace、pnpm、OpenSpec。

**Spec:**
- `docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md`
- `openspec/changes/ci-pr-under-10-min/{proposal.md,design.md,tasks.md,specs/engineering-governance/spec.md}`

## Global Constraints

- Issue #124；分支 `codex/issue-124-ci-pr-under-10-min`；禁止直接推送 `main`。
- 523 场景 / 541 证据 / 43 个 task / 注入证明数量只增不减。
- 不修改产品代码；不改变既有 spec 的 WHEN / THEN 语义。
- 不放宽 `-D warnings`、coverage 90%、`scale-baseline.json`、build purity。
- 所有 `cargo test --list` 调用必须带 `--all-targets`；若某包不支持，退回 `--lib --tests --all-features`。
- 本地跑 `pnpm verify:governance` 必须独占运行；不要同时跑 cargo、`rg` 或其它 node 扫描（5 秒墙钟断言会假红）。
- 单场景验证用 `node scripts/verify/run-scenario.mjs <id>`；不要用 `pnpm verify:scenario -- <id>`（`--` 会被当作场景 id）。
- 提交信息用中文；PR 关联 Issue #124。
- 每完成一组任务更新 `openspec/changes/ci-pr-under-10-min/tasks.md` 的勾选状态。

## Review Focus

- **PR 事件下的 Windows/macOS `Test`**：必须跳过完整测试，但 `fmt` / `clippy` / 平台 Gate 仍执行；`if` 若漏掉 `github.event_name != 'pull_request'`，main 也会跳过测试。由 `task-1.5` 的步骤断言与 `ci-structure/test-condition-windows` 注入证明覆盖。
- **`--sections` 选择遗漏**：未知 section、空选择、从 CI 删除 `coverage` 都必须失败，不能静默少跑一段。由 `ci-governance.mjs` 的参数校验与 `ci-structure/section-coverage-missing` 注入证明覆盖。
- **多 gate `--delegated`**：`governance,perf` 必须同时覆盖两类场景；单 gate 仍兼容；flag 不得被当成场景 id。由 `self-test.mjs` 的多 gate fixture 覆盖。
- **注入分片公式**：并集必须等于全集、各片互斥；越界或空片必须失败，不能静默漏跑条目。由 `injection-shard.mjs` 的运行时断言与 `--shard=3/2` 用例覆盖。
- **task 分片并集**：42 项 + Rust Linux 的 `16.1` 恰好 43 项；遗漏、重复、把 `16.1` 留在 task-gates 都必须失败。由 `task-1.5` 的固定清单断言覆盖。
- **`--all-targets` 的边界**：没有 bench/example 的包、或某个 example 在 `--list` 下不可用时，退回 `--lib --tests --all-features`；不能让列表命令重新构建 doc-tests。由 Task 1 的计时验证覆盖。

---

## File Structure

| 文件 | 责任 |
|---|---|
| `scripts/verify/validate-scenario-commands.mjs` | 场景证据静态校验；`cargo test --list` 对齐 `--all-targets` |
| `scripts/verify/check-scenario-evidence.mjs` | 证据覆盖门；`--all-targets` + `perf` 委托 |
| `scripts/verify/ci-governance.mjs` | 治理门编排；section 字段与 `--sections` / `--list-sections` |
| `scripts/verify/run-scenario.mjs` | 场景执行；多 gate `--delegated` |
| `scripts/verify/checks/task-13.9.mjs` | 发布门；`--skip-static` |
| `scripts/verify/scenario-commands.mjs` | 场景注册表；三条性能场景 `delegatedTo: "perf"` |
| `scripts/verify/manifest.json` | 由 `gen-scenario-manifests.mjs --write` 生成，不手工重排 |
| `scripts/verify/self-test.mjs` | runner 自测；fake PATH、多 gate fixture |
| `scripts/verify/checks/task-1.5.mjs` | CI 结构门禁；步骤级断言、43 项分解、section 覆盖 |
| `scripts/verify/injection-shard.mjs` | 新 helper；分片选择与并集/互斥校验 |
| `scripts/verify/injection-suite.mjs` | 注入套件；`--shard` 解析、嵌入条目 `shard: 1` |
| `scripts/verify/injection-governance.mjs` | 三条新注入证明 |
| `.github/workflows/ci.yml` | job 图；18 个 job、矩阵条件、步骤顺序 |
| `package.json` | 新增 `verify:perf` |
| `docs/arch-standards-remaining.md` | 维护者文档：job 拓扑、section、分片、分层 |
| `docs/ci-pipeline-review-2026-10-28.md` | 只追加「后续」小节，不改历史结论 |
| `docs/native-attestation-playbook.md` | 委托 `perf` 与 `--skip-static` 说明 |

---

## Task 1: 对齐 `cargo test --list` 的构建形态

**Files:**
- Modify: `scripts/verify/validate-scenario-commands.mjs:36`
- Modify: `scripts/verify/check-scenario-evidence.mjs:60`

**Interfaces:**
- Consumes: 无。
- Produces: 两个脚本的 `cargoTestList(packageName)` 行为不变，只是 cargo 参数增加 `--all-targets`。

- [ ] **Step 1: 记录基线**

Run:
```bash
mkdir -p artifacts
{
  /usr/bin/time -p pnpm verify:governance -- --without-injection
  /usr/bin/time -p pnpm verify:task -- --all
  node scripts/verify/injection-suite.mjs --list | wc -l
  gh run view 41323932057 --json jobs
} > artifacts/ci-pr-under-10-min-baseline.txt 2>&1
test -s artifacts/ci-pr-under-10-min-baseline.txt
```
Expected: 文件包含 governance、task、injection 条目数与 run `41323932057` 的 job 列表；`artifacts/` 已被忽略。

- [ ] **Step 2: 观察当前行为**

Run:
```bash
/usr/bin/time -p cargo test -p echo-core --all-features -- --list
```
Expected: 输出包含 `Doc-tests echo_core`；冷构建约 20s+。把耗时追加到 `artifacts/ci-pr-under-10-min-baseline.txt`。

- [ ] **Step 3: 修改两个脚本**

把两处：
```js
["test", "-p", packageName, "--all-features", "--", "--list"]
```
改为：
```js
["test", "-p", packageName, "--all-targets", "--all-features", "--", "--list"]
```
若某包在 `--all-targets` 下无法列出（bench/example 报错），改为：
```js
["test", "-p", packageName, "--lib", "--tests", "--all-features", "--", "--list"]
```
并在脚本注释里写明该包为什么需要回退。

- [ ] **Step 4: 验证热构建成本**

Run:
```bash
cargo test --workspace --all-targets --all-features --no-run
/usr/bin/time -p node scripts/verify/validate-scenario-commands.mjs
/usr/bin/time -p node scripts/verify/check-scenario-evidence.mjs
```
Expected: 两个脚本都退出 0；各自 < 60s；输出不含 `Doc-tests` 构建行。

- [ ] **Step 5: 提交**

```bash
git add scripts/verify/validate-scenario-commands.mjs scripts/verify/check-scenario-evidence.mjs
git commit -m "fix(verify): 对齐 cargo test --list 的 --all-targets 构建形态"
```

---

## Task 2: `ci-governance.mjs` 增加 section 与 `--sections`

**Files:**
- Modify: `scripts/verify/ci-governance.mjs`

**Interfaces:**
- Consumes: 无。
- Produces:
  - 每个 check 元组变成 `[label, command, args, section]`。
  - `--list-sections` 按固定顺序输出：`quality registry rust-static static coverage purity embedded injection`。
  - `--sections=a,b` 只运行选中的 section；未知 section / 空选择 / 语法错误非零退出。
  - `--without-injection` 仍然排除 `injection` section。

- [ ] **Step 1: 给检查表加 section**

按以下映射给每条 check 加第四个元素：

| Section | Checks |
|---|---|
| `quality` | `quality-core format`、`quality-core lint`、`quality-core tests` |
| `registry` | `scenario reconciliation`、`scenario registry drift` |
| `rust-static` | `scenario command validity`、`scenario evidence coverage` |
| `static` | `scale limits`、`fixture determinism and licensing`、`lint inheritance`、`toolchain pin`、`verification validity`、`scenario command proof` |
| `coverage` | `coverage gate` |
| `purity` | `build purity` |
| `embedded` | `embedded frontend freshness` |
| `injection` | `gate injection proofs`（仅在未传 `--without-injection` 时加入） |

把文件底部 `checks.push(["gate injection proofs", "node", ["scripts/verify/injection-suite.mjs"]]);` 改为 `checks.push(["gate injection proofs", "node", ["scripts/verify/injection-suite.mjs"], "injection"]);`。

- [ ] **Step 2: 实现 CLI 解析**

在 `checks` 定义之后、执行循环之前加入：
```js
const SECTION_ORDER = ["quality", "registry", "rust-static", "static", "coverage", "purity", "embedded", "injection"];
const argv = process.argv.slice(2);
if (argv.includes("--list-sections")) {
  process.stdout.write(`${SECTION_ORDER.join(" ")}\n`);
  process.exit(0);
}
const sectionsFlag = argv.find((arg) => arg.startsWith("--sections="));
let selectedSections = null;
if (sectionsFlag) {
  const raw = sectionsFlag.slice("--sections=".length);
  const requested = raw.split(",").map((s) => s.trim()).filter(Boolean);
  if (!requested.length) {
    process.stderr.write("FAIL governance: --sections requires at least one section\n");
    process.exit(1);
  }
  const unknown = requested.filter((s) => !SECTION_ORDER.includes(s));
  if (unknown.length) {
    process.stderr.write(`FAIL governance: unknown section(s): ${unknown.join(", ")}\n`);
    process.exit(1);
  }
  selectedSections = new Set(requested);
}
const withoutInjection = argv.includes("--without-injection");
const runnable = checks.filter(([, , , section]) =>
  (!selectedSections || selectedSections.has(section))
  && !(withoutInjection && section === "injection"));
if (!runnable.length) {
  process.stderr.write("FAIL governance: section selection produced no checks\n");
  process.exit(1);
}
```
把执行循环从 `for (const [label, command, args] of checks)` 改为遍历 `runnable`。

- [ ] **Step 3: 验证**

Run:
```bash
node scripts/verify/ci-governance.mjs --list-sections
node scripts/verify/ci-governance.mjs --sections=registry
node scripts/verify/ci-governance.mjs --sections=no-such
node scripts/verify/ci-governance.mjs --without-injection
```
Expected:
- 第一条输出 8 个 section。
- 第二条退出 0，只出现 `scenario reconciliation` / `scenario registry drift`。
- 第三条非零退出并列出 `no-such`。
- 第四条退出 0，不运行 injection。

- [ ] **Step 4: 提交**

```bash
git add scripts/verify/ci-governance.mjs
git commit -m "feat(verify): governance 支持 section 选择与 --list-sections"
```

---

## Task 3: 性能场景委托与多 gate `--delegated`

**Files:**
- Modify: `scripts/verify/scenario-commands.mjs:148,406,464`
- Modify: `scripts/verify/check-scenario-evidence.mjs:42`
- Modify: `scripts/verify/run-scenario.mjs:182-275`
- Modify: `scripts/verify/self-test.mjs:138-200,279-290`
- Modify: `package.json:10-18`

**Interfaces:**
- Consumes: Task 2 的 section 选择不影响本任务。
- Produces:
  - `SCENARIOS[id].delegatedTo` 允许 `"governance"` 与 `"perf"`。
  - `run-scenario.mjs --delegated=a,b` 接受逗号分隔；只跳过 `delegatedTo` 在集合中的场景。
  - `pnpm verify:perf` 运行三条性能场景。
  - `check-scenario-evidence` 要求 `pnpm verify:perf` 出现在 `ci.yml`。

- [ ] **Step 1: 写失败的 self-test 断言**

在 `self-test.mjs` 的 `scenarioFixture.scenarios` 中加入：
```js
{
  id: "rust-perf-test",
  title: "performance test explicitly delegated to the perf job",
  command: "cargo test -p echo-core --all-features perf-filter",
  delegatedTo: "perf",
  evidence: [{ kind: "rust", target: "perf_filter_ok" }],
},
```
在 fake cargo 的 `case` 中加入：
```sh
*perf-filter*) printf 'test perf_filter_ok ... ok\n'; passed=1 ;;
```
在 `reusedRustScenarios` 之后加入：
```js
const multiGateScenarios = runScenarioRunner(
  scenarioFixture,
  ["--delegated=governance,perf", "rust-reused-test", "rust-perf-test"],
  scenarioEnv,
);
assert(
  multiGateScenarios.status === 0
    && multiGateScenarios.stdout.includes("covered by the governance job")
    && multiGateScenarios.stdout.includes("covered by the perf job"),
  "comma-separated delegated gates cover both scenarios",
);
const singlePerfGate = runScenarioRunner(
  scenarioFixture,
  ["--delegated=perf", "rust-reused-test", "rust-perf-test"],
  scenarioEnv,
);
assert(
  singlePerfGate.status !== 0
    && singlePerfGate.stdout.includes("covered by the perf job")
    && singlePerfGate.stderr.includes("rust-reused-test"),
  "a single delegated gate covers only its declared scenarios",
);
```
同时把文件末尾的 `runRunner(fixture, ["ok", "failing", "--all"])` 改为 `runRunner(fixture, ["ok", "failing", "--all"], taskEnv)`，让 lockfile 不变性用例也使用 fake PATH。

- [ ] **Step 2: 运行 self-test，确认失败**

Run: `pnpm verify:self-test`
Expected: 新用例失败；`run-scenario.mjs` 把 `"governance,perf"` 当成一个 gate，两个场景都没有被覆盖。

- [ ] **Step 3: 实现多 gate 解析**

在 `run-scenario.mjs` 中把 `delegatedGate` 改为集合：
```js
const delegatedFlag = rawArgv.find((arg) => arg.startsWith("--delegated="));
const delegatedGates = delegatedFlag
  ? new Set(delegatedFlag.slice("--delegated=".length).split(",").map((s) => s.trim()).filter(Boolean))
  : null;
if (delegatedFlag && (!delegatedGates || delegatedGates.size === 0)) {
  fail("--delegated=<gate> requires at least one gate name");
  process.exit(1);
}
```
`executeScenarios(requested, delegatedGates)` 中：
```js
if (scenario && delegatedGates !== null && delegatedGates.has(scenario.delegatedTo)) {
  covered.add(id);
  continue;
}
```
输出改为：
```js
process.stdout.write(`ok: scenario ${id} (${scenario.title}) — covered by the ${scenario.delegatedTo} job\n`);
```

- [ ] **Step 4: 加入 perf 委托**

在 `scenario-commands.mjs` 的三条性能场景条目上加 `delegatedTo: "perf"`：
```js
"SFI-R05-S03": { command: PERF_ALL, delegatedTo: "perf", evidence: [rust("completed_local_edit_latency")], },
"LE-R21-S02": { command: PERF_ALL, delegatedTo: "perf", evidence: [rust("completed_local_edit_latency")], },
"PM-R13-S05": { command: PERF_ALL, delegatedTo: "perf", evidence: [rust("completed_local_edit_latency")], },
```
把 `check-scenario-evidence.mjs` 的：
```js
const DELEGATED_GATES = new Set(["governance"]);
```
改为：
```js
const DELEGATED_GATES = new Set(["governance", "perf"]);
```
在 `package.json` 的 `scripts` 中加入：
```json
"verify:perf": "node scripts/verify/run-scenario.mjs SFI-R05-S03 LE-R21-S02 PM-R13-S05"
```

- [ ] **Step 5: 重新生成 manifest 并验证**

Run:
```bash
node scripts/verify/gen-scenario-manifests.mjs --write
node -e "const m=require('./scripts/verify/manifest.json');console.log(m.scenarios.filter(s=>s.delegatedTo==='perf').map(s=>s.id).join(' '))"
pnpm verify:self-test
/usr/bin/time -p node scripts/verify/checks/task-1.3.mjs
node scripts/verify/run-scenario.mjs --delegated=governance,perf LD-R10-S01 PHA-R01-S02 SFI-R05-S03
```
Expected:
- manifest 中三条 perf ID。
- self-test 全绿；`task-1.3` 退出 0 且 < 30s（lockfile SHA-256 断言仍在）。
- 三条场景都显示 covered。
- `check-scenario-evidence` 现在会要求 `ci.yml` 出现 `pnpm verify:perf`，因此它的绿灯在 Task 8 接入 perf job 后验证。

- [ ] **Step 6: 提交**

```bash
git add scripts/verify/scenario-commands.mjs scripts/verify/manifest.json scripts/verify/check-scenario-evidence.mjs scripts/verify/run-scenario.mjs scripts/verify/self-test.mjs package.json
git commit -m "feat(verify): 支持多 gate 委托并接入性能场景"
```

---

## Task 4: `task-13.9` 增加 `--skip-static`

**Files:**
- Modify: `scripts/verify/checks/task-13.9.mjs:76-96,105-136`

**Interfaces:**
- Consumes: Task 3 的 `--delegated=governance,perf`。
- Produces: `--skip-static` 跳过 reconcile / validate / gen；场景执行与 PRD matrix 保留。

- [ ] **Step 1: 实现参数**

在文件顶部解析：
```js
const argv = process.argv.slice(2);
const skipStatic = argv.includes("--skip-static");
const delegated = argv.find((arg) => arg.startsWith("--delegated="));
const delegatedArgs = delegated ? [delegated] : [];
```
删除第 106–107 行原有的 `const delegated = ...` / `const delegatedArgs = ...` 声明，避免重复声明。

把第 1、2 步的 `run(...)` 包在 `if (!skipStatic) { ... }` 中：
```js
if (!skipStatic) {
  const reconcileOut = run("node", ["scripts/verify/reconcile-scenarios.mjs"]).trim();
  // ...现有 reconcile 断言...
  run("node", ["scripts/verify/validate-scenario-commands.mjs"]);
  run("node", ["scripts/verify/gen-scenario-manifests.mjs"]);
}
```
最终输出：
```js
const summary = skipStatic
  ? `ok 13.9: static checks skipped (delegated); automated scenario suite ${okLines}/${expectedAutomatedCount}; PRD A1–A14 matrix verified`
  : `ok 13.9: specs == traceability == manifest (${expectedScenarioCount} scenarios); automated scenario suite ${okLines}/${expectedAutomatedCount}; PRD A1–A14 matrix verified`;
process.stdout.write(`${summary}\n`);
```
注意：把 `const expectedScenarioCount = allScenarioIds().length;` 和 `mkdirSync(ARTIFACTS, { recursive: true });` 移到 `if (!skipStatic)` 之外（它们不依赖 reconcile）；`--skip-static` 时输出不得声称 `specs == traceability == manifest`。

- [ ] **Step 2: 验证**

Run:
```bash
node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf
node scripts/verify/checks/task-13.9.mjs --delegated=governance,perf
```
Expected: 第一条退出 0 且输出包含 `static checks skipped`；第二条仍执行静态校验并退出 0。

- [ ] **Step 3: 提交**

```bash
git add scripts/verify/checks/task-13.9.mjs
git commit -m "feat(verify): task-13.9 支持 --skip-static"
```

---

## Task 5: 扩展 `task-1.5` 的结构门禁（先红）

**Files:**
- Modify: `scripts/verify/checks/task-1.5.mjs`（当前 77 行，预计 ~220 行）

**Interfaces:**
- Consumes: Task 2 的 `--list-sections`。
- Produces: 对 `ci.yml` 的以下断言（Task 6–9 将让它们变绿）：
  - `rust` job 的 `Test`、`Rust static gates`、`Task 16.1`、`Task 9.7 9.8` 的 `run` / `if` / `env`。
  - `task-gates` 的 4 个 shard、固定 task 清单、42 项并集。
  - `governance-static`、`perf`、`release-gate`、injection 两片的接线。
  - section 全覆盖。

- [ ] **Step 1: 加 YAML 步骤定位 helper**

在现有 `workflow-matrix.mjs` 导入之后加：
```js
function step(workflow, jobId, name) {
  const steps = workflow?.jobs?.[jobId]?.steps;
  if (!Array.isArray(steps)) fail(`ci.yml job '${jobId}' has no steps`);
  const found = steps.find((s) => s.name === name);
  if (!found) fail(`ci.yml job '${jobId}' is missing step '${name}'`);
  return found;
}
function assertRunIncludes(s, needle, label) {
  if (!String(s.run || "").includes(needle)) fail(`${label} run must include '${needle}'`);
}
function assertIfIncludes(s, needle, label) {
  if (!String(s.if || "").includes(needle)) fail(`${label} if must include '${needle}'`);
}
function assertEnv(s, key, value, label) {
  if (String(s.env?.[key]) !== value) fail(`${label} env ${key} must be '${value}'`);
}
function sectionsOf(run) {
  const match = String(run || "").match(/--sections=([a-z,-]+)/);
  return match ? match[1].split(",") : [];
}
```

- [ ] **Step 2: 加步骤断言**

先把 `REQUIRED` 中的 `"cargo test --workspace --all-features"` 改为 `"cargo test --workspace --all-targets --all-features"`，与新的 `Test` 步骤一致。

在现有 `REQUIRED` 循环之后加入：
```js
const test = step(workflow, "rust", "Test");
assertIfIncludes(test, "github.event_name != 'pull_request'", "rust Test");
assertIfIncludes(test, "matrix.platform == 'Linux'", "rust Test");
if (test.run !== "cargo test --workspace --all-targets --all-features") {
  fail("rust Test run must be 'cargo test --workspace --all-targets --all-features'");
}

const format = step(workflow, "rust", "Format check");
if (format.run !== "cargo fmt --all -- --check") fail("rust Format check run must be 'cargo fmt --all -- --check'");
const clippy = step(workflow, "rust", "Clippy");
if (clippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}

const rustStatic = step(workflow, "rust", "Rust static gates");
assertIfIncludes(rustStatic, "matrix.platform == 'Linux'", "Rust static gates");
assertRunIncludes(rustStatic, "--sections=rust-static,purity", "Rust static gates");
assertEnv(rustStatic, "ECHO_PURITY_REQUIRE_BUILD", "1", "Rust static gates");

const task161 = step(workflow, "rust", "Task 16.1");
assertIfIncludes(task161, "matrix.platform == 'Linux'", "Task 16.1");
if (task161.run !== "pnpm verify:task -- 16.1") fail("Task 16.1 run must be 'pnpm verify:task -- 16.1'");

const task978 = step(workflow, "rust", "Task 9.7 9.8");
assertIfIncludes(task978, "matrix.platform == 'Linux'", "Task 9.7 9.8");
if (task978.run !== "pnpm verify:task -- 9.7 9.8") fail("Task 9.7 9.8 run must be 'pnpm verify:task -- 9.7 9.8'");

const governanceStatic = step(workflow, "governance-static", "Run governance static gates");
// 只断言这里存在 section 选择；具体覆盖由 Step 4 的权威 section 列表校验。
assertRunIncludes(governanceStatic, "--sections=", "governance-static");

const release = step(workflow, "release-gate", "Run release gate without external artifacts");
if (release.run !== "node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf") {
  fail("release-gate run must use --skip-static --delegated=governance,perf");
}

const perf = step(workflow, "perf", "Run performance scenarios");
if (perf.run !== "pnpm verify:perf") fail("perf run must be 'pnpm verify:perf'");

const injection = workflow.jobs["injection-proofs"];
if (JSON.stringify(injection?.strategy?.matrix?.shard) !== JSON.stringify([1, 2])) {
  fail("injection-proofs matrix.shard must be [1, 2]");
}
const shardStep = step(workflow, "injection-proofs", "Prove each governance gate");
assertRunIncludes(shardStep, "--shard=${{ matrix.shard }}/2", "injection shard step");
const shellStep = step(workflow, "injection-proofs", "Build the shell binary checked by embedded-frontend proof");
assertIfIncludes(shellStep, "matrix.shard == 1", "shell build step");
const embeddedStep = step(workflow, "injection-proofs", "Verify embedded frontend freshness");
assertIfIncludes(embeddedStep, "matrix.shard == 1", "embedded freshness step");
if (embeddedStep.run !== "pnpm verify:frontend-fresh") fail("embedded freshness run must be 'pnpm verify:frontend-fresh'");
```

- [ ] **Step 3: 加 task 分片断言**

在文件顶部定义固定清单：
```js
const EXPECTED_SHARDS = {
  core: "1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3".split(" "),
  desktop: "7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2".split(" "),
  "frontend-a": "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1".split(" "),
  "frontend-b": "10.7 10.8 10.9 10.10 12.1 12.2 12.3".split(" "),
};
```
加入断言：
```js
const matrix = workflow.jobs["task-gates"]?.strategy?.matrix;
if (!matrix || Object.keys(matrix).some((key) => key !== "include")) {
  fail("task-gates must use an include-only matrix");
}
const shards = matrix.include || [];
if (shards.length !== 4) fail(`task-gates must have 4 shards, got ${shards.length}`);
const byShard = new Map(shards.map((entry) => [entry.shard, String(entry.tasks || "").split(/\s+/).filter(Boolean)]));
for (const [name, expected] of Object.entries(EXPECTED_SHARDS)) {
  const actual = byShard.get(name);
  if (!actual) fail(`task-gates is missing shard '${name}'`);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`task-gates shard '${name}' must be [${expected.join(" ")}], got [${actual.join(" ")}]`);
  }
}
const union = [...byShard.values()].flat();
if (union.length !== 42) fail(`task-gates shard union must be 42 tasks, got ${union.length}`);
if (new Set(union).size !== union.length) fail("task-gates shard union contains duplicates");
if (union.includes("16.1")) fail("16.1 must run in Rust Linux, not task-gates");
```

- [ ] **Step 4: 加 section 全覆盖断言**

在文件顶部导入 `spawnSync`：
```js
import { spawnSync } from "node:child_process";
```
加入：
```js
const listed = spawnSync(process.execPath, [resolve(ROOT, "scripts", "verify", "ci-governance.mjs"), "--list-sections"], {
  cwd: ROOT,
  encoding: "utf8",
});
if (listed.status !== 0) fail(`ci-governance.mjs --list-sections exited ${listed.status}`);
const sections = listed.stdout.trim().split(/\s+/).filter(Boolean);
const covered = new Set([
  "quality", // rust 矩阵的 fmt/clippy/Test
  ...sectionsOf(step(workflow, "rust", "Rust static gates").run),
  ...sectionsOf(step(workflow, "governance-static", "Run governance static gates").run),
  "embedded", // verify:frontend-fresh
  "injection", // injection-suite --shard
]);
for (const section of sections) {
  if (!covered.has(section)) fail(`ci-governance section '${section}' has no execution location in ci.yml`);
}
```

- [ ] **Step 5: 运行，确认是红的**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: 非零退出；当前 `ci.yml` 先在 `REQUIRED` 的 `cargo test --workspace --all-targets --all-features` 上失败，修掉这一项后才会依次暴露缺失的 `Rust static gates` / `governance-static` / 4 个 shard。

- [ ] **Step 6: 提交（红测试单独提交）**

```bash
git add scripts/verify/checks/task-1.5.mjs
git commit -m "test(verify): 扩展 task-1.5 的 CI 结构断言（预期红）"
```

---

## Task 6: `task-gates` 拆成 4 个 shard

**Files:**
- Modify: `.github/workflows/ci.yml:140-211`

**Interfaces:**
- Consumes: Task 5 的 `EXPECTED_SHARDS`。
- Produces: `task-gates` 的 include-only matrix，4 个 shard，每个 shard 的 `tasks` 是空格分隔字符串。

- [ ] **Step 1: 重写 job**

```yaml
  task-gates:
    name: Task gates (${{ matrix.shard }})
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        include:
          - shard: core
            tasks: "1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3"
          - shard: desktop
            tasks: "7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2"
          - shard: frontend-a
            tasks: "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1"
          - shard: frontend-b
            tasks: "10.7 10.8 10.9 10.10 12.1 12.2 12.3"
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust toolchain (pinned, rustfmt + clippy)
        if: matrix.shard == 'core' || matrix.shard == 'desktop'
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
          components: rustfmt, clippy
      - name: Cache Rust build artifacts
        if: matrix.shard == 'core' || matrix.shard == 'desktop'
        uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version-file: .nvmrc
      - name: Install pnpm
        uses: pnpm/action-setup@v4
        with:
          version: 11.18.0
      - name: Install dependencies
        run: pnpm install --frozen-lockfile
      - name: Verify task gates
        env:
          CHROME_PATH: /usr/bin/google-chrome
          CHROME_NO_SANDBOX: "1"
        run: pnpm verify:task -- ${{ matrix.tasks }}
      - name: Upload 50k benchmark result
        if: matrix.shard == 'core'
        uses: actions/upload-artifact@v4
        with:
          name: benchmark-50k
          path: apps/desktop/benchmark-50k.json
          if-no-files-found: error
```

- [ ] **Step 2: 验证 task 分片**

Run:
```bash
node --input-type=module -e "import {readFileSync} from 'node:fs';import yaml from 'js-yaml';const w=yaml.load(readFileSync('.github/workflows/ci.yml','utf8'));const inc=w.jobs['task-gates'].strategy.matrix.include;console.log(inc.map(e=>[e.shard,e.tasks.split(/\\s+/).length]));"
node scripts/verify/checks/task-1.5.mjs
```
Expected: 输出 4 个 shard 的 task 数 10 / 17 / 8 / 7；`task-1.5` 仍然红在 `REQUIRED` 的 `cargo test --workspace --all-targets --all-features` 上（Test 步骤尚未改），task 分片断言要等 Task 7 改完 Test 后才会被执行。

- [ ] **Step 3: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: task gates 拆为 4 个显式 shard"
```

---

## Task 7: Rust 矩阵分层与 Linux 专属步骤

**Files:**
- Modify: `.github/workflows/ci.yml:13-95,337-367`

**Interfaces:**
- Consumes: Task 5 的步骤断言。
- Produces: `rust` job 的新步骤；`linux-platform-gate` 删除；LFS 只在 Windows checkout。

- [ ] **Step 1: 改 checkout 与 Test**

把 `rust` job 的 checkout 改为：
```yaml
      - uses: actions/checkout@v4
        with:
          lfs: ${{ runner.os == 'Windows' }}
```
把 `Test` 改为：
```yaml
      - name: Test
        if: github.event_name != 'pull_request' || matrix.platform == 'Linux'
        run: cargo test --workspace --all-targets --all-features
```

- [ ] **Step 2: 加 Linux 专属步骤**

在 `Test` 之后、macOS 的 8.12/13.2 步骤之前加入：
```yaml
      - name: Rust static gates
        if: matrix.platform == 'Linux'
        env:
          ECHO_PURITY_REQUIRE_BUILD: "1"
        run: pnpm verify:governance -- --sections=rust-static,purity

      - name: Task 16.1
        if: matrix.platform == 'Linux'
        run: pnpm verify:task -- 16.1

      - name: Task 9.7 9.8
        if: matrix.platform == 'Linux'
        run: pnpm verify:task -- 9.7 9.8
```

- [ ] **Step 3: 删除 `linux-platform-gate` job**

删除整个 `linux-platform-gate` job（当前 `ci.yml:337-367`）。

- [ ] **Step 4: 验证**

Run:
```bash
rg -n "linux-platform-gate" .github/workflows/ci.yml
rg -n "lfs: true" .github/workflows/ci.yml
node scripts/verify/checks/task-1.5.mjs
```
Expected:
- 第一条无输出。
- 第二条只命中 Windows platform Gate（以及 `lfs: ${{ runner.os == 'Windows' }}` 不是 `lfs: true`）。
- `task-1.5` 仍然红，失败点前移到 `governance-static`。
- `Rust static gates` 的实际执行要等 Task 8 接入 perf job 后再验证：`check-scenario-evidence` 会要求 `ci.yml` 出现 `pnpm verify:perf`。

- [ ] **Step 5: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: PR 只在 Linux 跑完整测试，Linux 平台门并入 Rust Linux"
```

---

## Task 8: Governance static、Release gate 与 Performance job

**Files:**
- Modify: `.github/workflows/ci.yml:369-433,479-516`

**Interfaces:**
- Consumes: Task 2 的 `--sections`；Task 3 的 `verify:perf`；Task 4 的 `--skip-static`。
- Produces: `governance-static`、`perf`、新的 `release-gate` 命令。

- [ ] **Step 1: 重写 governance job**

把 `governance` job 改名为 `governance-static`，删除 webkit 安装与前端构建步骤，保留 ffmpeg、coverage tooling、node/pnpm、依赖安装；把最后一步改为：
```yaml
      - name: Run governance static gates
        run: pnpm verify:governance -- --sections=registry,static,coverage
```
删除独立的 `Verify active Rust toolchain` 步骤（`toolchain pin` 已在 `static` section 中执行）。

- [ ] **Step 2: 改 release gate**

把 `release-gate` 的最后一步改为：
```yaml
      - name: Run release gate without external artifacts
        run: node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf
```

- [ ] **Step 3: 新增 perf job**

在 `release-gate` 之后加入：
```yaml
  perf:
    name: Performance scenarios
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust toolchain (pinned)
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
      - name: Cache Rust build artifacts
        uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version-file: .nvmrc
      - name: Install pnpm
        uses: pnpm/action-setup@v4
        with:
          version: 11.18.0
      - name: Run performance scenarios
        run: pnpm verify:perf
```

- [ ] **Step 4: 验证**

Run:
```bash
rg -n "governance-static|--sections=registry,static,coverage" .github/workflows/ci.yml
rg -n "verify:perf|--skip-static|delegated=governance,perf" .github/workflows/ci.yml package.json
node scripts/verify/check-scenario-evidence.mjs
node scripts/verify/ci-governance.mjs --sections=registry,static,coverage
node scripts/verify/ci-governance.mjs --sections=rust-static,purity
node scripts/verify/checks/task-1.5.mjs
```
Expected:
- 前两条命中。
- evidence 门退出 0。
- 两个 section 选择都退出 0（此时 perf job 已在 `ci.yml` 中，`check-scenario-evidence` 的 `pnpm verify:perf` 断言满足）。
- `task-1.5` 仍然红，但只剩 injection 相关断言（说明 governance/release/perf 已通过）。

- [ ] **Step 5: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: governance 分段、release gate 去静态、性能场景独立"
```

---

## Task 9: 注入证明拆成 2 个 shard

**Files:**
- Create: `scripts/verify/injection-shard.mjs`
- Modify: `scripts/verify/injection-suite.mjs:581-603,760-767`
- Modify: `.github/workflows/ci.yml:436-477`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `selectEntries(entries, { only, shard })` 返回当前片的条目；每次调用校验并集/互斥。
  - `injection-suite.mjs --shard=<i>/<n>`。
  - `injection-proofs` 的 `matrix.shard: [1, 2]`。

- [ ] **Step 1: 写 helper**

`scripts/verify/injection-shard.mjs`：
```js
export function selectEntries(entries, { only = null, shard = null } = {}) {
  const filtered = only ? entries.filter((entry) => entry.id.includes(only)) : entries;
  if (shard === null) return filtered;

  const match = /^(\d+)\/(\d+)$/.exec(shard);
  if (!match) throw new Error(`--shard must look like <i>/<n>, got '${shard}'`);
  const index = Number(match[1]);
  const total = Number(match[2]);
  if (total < 1 || index < 1 || index > total) {
    throw new Error(`--shard ${shard} is out of range`);
  }

  const buckets = Array.from({ length: total }, () => []);
  filtered.forEach((entry, position) => {
    const assigned = entry.shard ?? (position % total) + 1;
    if (assigned < 1 || assigned > total) {
      throw new Error(`entry '${entry.id}' declares shard ${assigned}, outside 1..${total}`);
    }
    buckets[assigned - 1].push(entry);
  });

  const union = buckets.flat();
  if (union.length !== filtered.length || new Set(union).size !== filtered.length) {
    throw new Error("injection shards must be disjoint and cover every selected entry");
  }
  for (let i = 0; i < total; i += 1) {
    if (buckets[i].length === 0) throw new Error(`injection shard ${i + 1}/${total} is empty`);
  }
  return buckets[index - 1];
}
```

- [ ] **Step 2: 接入 suite**

在 `injection-suite.mjs` 导入：
```js
import { selectEntries } from "./injection-shard.mjs";
```
把：
```js
const selected = only ? ENTRIES.filter((entry) => entry.id.includes(only)) : ENTRIES;
```
改为：
```js
const shardIndex = argv.findIndex((arg) => arg.startsWith("--shard="));
const shard = shardIndex === -1 ? null : argv[shardIndex].slice("--shard=".length);
let selected;
try {
  selected = selectEntries(ENTRIES, { only, shard });
} catch (error) {
  process.stderr.write(`error injection-suite: ${error.message}\n`);
  process.exit(1);
}
```
`--list` 仍在选择之前，列出全部条目。

- [ ] **Step 3: 给嵌入条目加 `shard: 1`**

在 `embedded-frontend/stale-binary` 条目上加：
```js
shard: 1,
```

- [ ] **Step 4: 改 CI job**

把 `injection-proofs` 改为：
```yaml
  injection-proofs:
    name: Injection proof ${{ matrix.shard }}/2
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2]
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust toolchain (pinned)
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
      - name: Cache Rust build artifacts
        uses: Swatinem/rust-cache@v2
      - name: Install Linux webview dependencies
        run: >-
          sudo apt-get update && sudo apt-get install -y
          libwebkit2gtk-4.1-dev libayatana-appindicator3-dev
          librsvg2-dev patchelf
      - uses: actions/setup-node@v4
        with:
          node-version-file: .nvmrc
      - name: Install pnpm
        uses: pnpm/action-setup@v4
        with:
          version: 11.18.0
      - name: Install dependencies
        run: pnpm install --frozen-lockfile
      - name: Build the frontend bundle the desktop shell embeds
        run: pnpm --dir apps/desktop build
      - name: Build the shell binary checked by embedded-frontend proof
        if: matrix.shard == 1
        run: cargo build -p echo-app
      - name: Verify embedded frontend freshness
        if: matrix.shard == 1
        run: pnpm verify:frontend-fresh
      - name: Prove each governance gate
        env:
          ECHO_INJECTION_REQUIRE_ALL: "1"
        run: node scripts/verify/injection-suite.mjs --shard=${{ matrix.shard }}/2
```

- [ ] **Step 5: 验证**

Run:
```bash
node scripts/verify/injection-suite.mjs --list | wc -l
node scripts/verify/injection-suite.mjs --shard=1/2
node scripts/verify/injection-suite.mjs --shard=2/2
node scripts/verify/injection-suite.mjs --shard=3/2
node scripts/verify/checks/task-1.5.mjs
node scripts/verify/check-scale.mjs
```
Expected:
- 条目数与改动前一致。
- 两片都退出 0；`3/2` 非零退出。
- `task-1.5` 现在全绿。
- `check-scale` 退出 0（`injection-suite.mjs` 仍 < 1000 行；新增解析逻辑放进 helper，不继续堆在 suite 里）。

- [ ] **Step 6: 提交**

```bash
git add scripts/verify/injection-shard.mjs scripts/verify/injection-suite.mjs .github/workflows/ci.yml
git commit -m "ci: 注入证明拆为 2 个 shard"
```

---

## Task 10: 为 `task-1.5` 的新断言补注入证明

**Files:**
- Modify: `scripts/verify/injection-governance.mjs`

**Interfaces:**
- Consumes: Task 9 的 `injection-suite.mjs --shard`。
- Produces: 三条 `ci-structure/*` 证明，均让 `task-1.5.mjs` 失败。

- [ ] **Step 1: 加三条条目**

在 `governanceEntries` 返回数组的 `ci-platform-matrix/missing-linux-entry` 之后加入：
```js
{
  id: "ci-structure/test-condition-windows",
  guard: "the Rust Test step being gated to Windows instead of Linux on pull requests",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: rust Test if must include 'matrix\.platform == 'Linux''/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "if: github.event_name != 'pull_request' || matrix.platform == 'Linux'",
      "if: github.event_name != 'pull_request' || matrix.platform == 'Windows'",
    );
    return {};
  },
},
{
  id: "ci-structure/rust-static-step-missing",
  guard: "the Rust Linux job losing its rust-static/purity governance step",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: ci\.yml job 'rust' is missing step 'Rust static gates'/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "      - name: Rust static gates\n        if: matrix.platform == 'Linux'\n        env:\n          ECHO_PURITY_REQUIRE_BUILD: \"1\"\n        run: pnpm verify:governance -- --sections=rust-static,purity\n\n",
      "",
    );
    return {};
  },
},
{
  id: "ci-structure/section-coverage-missing",
  guard: "governance-static dropping the coverage section from its --sections selection",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: ci-governance section 'coverage' has no execution location in ci\.yml/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "--sections=registry,static,coverage",
      "--sections=registry,static",
    );
    return {};
  },
},
```
如果实际错误文案与 `expect` 不一致，以 `task-1.5` 的真实输出为准修改 `expect`，不要放宽断言。

- [ ] **Step 2: 验证**

Run:
```bash
node scripts/verify/injection-suite.mjs --only ci-structure
node scripts/verify/injection-suite.mjs --list | wc -l
```
Expected: 三条全部通过；条目数比 Task 1 基线增加 3。

- [ ] **Step 3: 提交**

```bash
git add scripts/verify/injection-governance.mjs
git commit -m "test(verify): 为 CI 结构断言补三条注入证明"
```

---

## Task 11: 更新维护者文档

**Files:**
- Modify: `docs/arch-standards-remaining.md`
- Modify: `docs/ci-pipeline-review-2026-10-28.md`
- Modify: `docs/native-attestation-playbook.md`

**Interfaces:**
- Consumes: Task 6–10 的最终结构。
- Produces: 文档与 `ci.yml` / 脚本一致。

- [ ] **Step 1: 更新架构文档**

在 `docs/arch-standards-remaining.md` 中记录：
- 18 个 job 的拓扑与关键路径；
- task 4 个 shard 与 42+1 分解；
- governance 8 个 section 与执行位置；
- PR/main 覆盖分层；
- `verify:perf`、`--skip-static`、多 gate `--delegated`；
- injection 2 个 shard 的轮转与显式 `shard`。

- [ ] **Step 2: 追加 CI review 后续**

在 `docs/ci-pipeline-review-2026-10-28.md` 末尾追加「后续（2026-11-02）」小节，只写本次改造与实测结果，不改历史结论。若 CI 尚未跑，先写设计与预期，Task 13 再补实测数字。

- [ ] **Step 3: 更新 attestation playbook**

在 `docs/native-attestation-playbook.md` 中说明：
- `LD-R10-S01` / `PHA-R01-S02` 的 `delegatedTo: "governance"`；
- 三条性能场景的 `delegatedTo: "perf"` 与 `pnpm verify:perf`；
- release gate 的 `--skip-static` 只跳过静态预检，不跳过场景。

- [ ] **Step 4: 验证**

Run:
```bash
rg -n "verify:perf|--skip-static|--list-sections|--delegated|injection.*shard" docs/arch-standards-remaining.md docs/native-attestation-playbook.md
rg -n "PR.*main|Windows.*macOS.*main|Rust static gates" docs/arch-standards-remaining.md docs/ci-pipeline-review-2026-10-28.md
```
Expected: 命中上述关键词；文档中的示例命令逐字执行退出 0。

- [ ] **Step 5: 提交**

```bash
git add docs/arch-standards-remaining.md docs/ci-pipeline-review-2026-10-28.md docs/native-attestation-playbook.md
git commit -m "docs: 记录 CI 分层、分片与委托契约"
```

---

## Task 12: 本地集成验证

**Files:**
- 无新增；只运行验证并修复回归。

**Interfaces:**
- Consumes: Task 1–11 的全部改动。
- Produces: 全绿的本地验证记录。

- [ ] **Step 1: 独占运行全量治理门**

Run:
```bash
/usr/bin/time -p pnpm verify:governance
```
Expected: 退出 0；记录墙钟，与 `artifacts/ci-pr-under-10-min-baseline.txt` 比较。不要在运行时并行跑其它 cargo / `rg` / node 扫描。

- [ ] **Step 2: task 与场景**

Run:
```bash
pnpm verify:task -- --all
node scripts/verify/run-scenario.mjs --automated
```
Expected: 都退出 0；43 个 task 全部执行；自动化场景数等于 manifest 减去 attestation 与 governance/perf 委托数。

- [ ] **Step 3: 静态门**

Run:
```bash
pnpm verify:self-test
node scripts/verify/checks/task-1.5.mjs
node scripts/verify/check-scenario-evidence.mjs
node scripts/verify/validate-scenario-commands.mjs
node scripts/verify/check-scale.mjs
```
Expected: 全部退出 0。

- [ ] **Step 4: 工作区干净**

Run:
```bash
git status --porcelain
```
Expected: 无输出；若有 `manifest.json` 或生成物变化，先确认是 `gen-scenario-manifests.mjs --write` 的预期结果再提交。

---

## Task 13: 归档、PR 与 CI 实测

**Files:**
- Modify: `openspec/specs/engineering-governance/spec.md`（通过归档同步）
- Modify: `docs/arch-standards-remaining.md` 或 `docs/ci-pipeline-review-2026-10-28.md`（实测数字）

**Interfaces:**
- Consumes: Task 12 的全绿本地验证。
- Produces: 已归档的 OpenSpec change、关联 #124 的 PR、CI 实测记录。

- [ ] **Step 1: 同步 delta 并归档**

使用 `openspec-archive-change` 技能（或等价命令）：
```bash
openspec validate ci-pr-under-10-min --strict
openspec validate --all --strict
```
先把 `openspec/changes/ci-pr-under-10-min/tasks.md` 中已完成的复选框改为 `[x]`，再把 `specs/engineering-governance/spec.md` 的 delta 同步到 `openspec/specs/engineering-governance/spec.md`，然后归档 change。归档后提交（包含设计文档与实施计划）：
```bash
git add docs/superpowers openspec
git commit -m "docs(openspec): 归档 ci-pr-under-10-min 并记录实施计划"
```
验证：`openspec list` 中不再有活动 change，`git status --porcelain` 为空。

- [ ] **Step 2: 推送分支并创建 PR**

先写 PR 正文：
```bash
cat > /tmp/echo-ci-pr.md <<'EOF'
Closes #124

## 目标
把 PR CI 墙钟从 21m46s 压到 10 分钟以内，不减少任何检查、场景、证据或 task。

## 主要改动
- task gates 拆为 4 个 shard（42 项），16.1 移到 Rust Linux
- PR 上 Windows/macOS 跳过完整 workspace 测试，main 仍执行
- governance 按 section 拆分，rust-static/purity 迁到 Rust Linux
- release gate 只跑场景，3 条性能场景独立为 perf job
- injection proofs 拆为 2 个 shard
- `cargo test --list` 统一带 `--all-targets`

## 覆盖映射
- 523 场景 / 541 证据 / 43 task / 注入证明数量均不减少
- `task-1.5` 断言 4 个 shard 并集 42 + Rust Linux 的 16.1 = 43
- 8 个 governance section 均有 CI 执行位置

## 设计
- `docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md`
- `openspec/changes/ci-pr-under-10-min/`

## 回滚
按 job 粒度回滚：先恢复 task-gates 单 job，再恢复 rust 矩阵的完整 Test，最后恢复 injection 单 job。
EOF
```

然后：
```bash
git push -u origin codex/issue-124-ci-pr-under-10-min
gh pr create --base main --head codex/issue-124-ci-pr-under-10-min \
  --title "ci: PR 墙钟压到 10 分钟内（job 去重、分片与平台分层）" \
  --body-file /tmp/echo-ci-pr.md
```
验证：`gh pr view --json number,body,url` 显示关联 #124。

- [ ] **Step 3: 记录 CI 实测**

Run:
```bash
gh run list --branch codex/issue-124-ci-pr-under-10-min --limit 1
gh run view <run-id> --json jobs
```
记录总墙钟与各 job/step 墙钟，重点：Rust (Linux) 的 `Test` / `rust-static` / 16.1 / purity、Governance static、Injection 1/2、Release gate、Performance。验收主口径是 rust-cache 命中的 run < 10 分钟；冷缓存 run 单独记录。

- [ ] **Step 4: 把实测数字写回文档并推送**

更新 `docs/arch-standards-remaining.md` 或 `docs/ci-pipeline-review-2026-10-28.md` 的「后续」小节，提交并推送：
```bash
git add docs/arch-standards-remaining.md docs/ci-pipeline-review-2026-10-28.md
git diff --cached --quiet || git commit -m "docs: 记录 PR CI 实测墙钟"
git push
```

- [ ] **Step 5: 合并并验证 main**

PR 全绿后合并（或交维护者合并）。在 `main` run 上确认：
```bash
gh run view <main-run-id> --json jobs
```
Expected: Windows / macOS 的 `Test` 步骤为 success（非 skipped）；记录 main 墙钟。
