# CI 运行耗时压缩（PR 与 main 双 10 分钟）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 PR 与 main 的 CI 墙钟都压到 10 分钟以内，并降低失败率，同时不删除任何检查、不放宽任何断言、不缩小平台覆盖。

**Architecture:** 先修缓存（10.13 GB 超过 GitHub Free 的 10 GB 上限导致三平台 Rust job 全部 miss，Test 步骤每次冷编译 318–413 个 crate），再拆关键路径（Windows/macOS 的 workspace 测试按包分两片、只分片不删包），最后收敛 cargo 形态（场景命令与任务脚本统一到 `--workspace --all-targets --all-features` 的 build 形态，使缓存可跨 job 复用）。CI 形状被 `task-1.5` / `injection-governance` 反向锁定，因此 workflow 与结构门禁必须在同一任务内同步改动。

**Tech Stack:** GitHub Actions、Swatinem/rust-cache@v2、Cargo、pnpm、Node 脚本门禁（`scripts/verify/**`）。

**Spec:** `docs/superpowers/specs/2026-11-16-ci-runtime-compression-design.md`（已批准）。执行者必须同时阅读该设计文档。

## Global Constraints

- **PR 与 main 的 CI 墙钟都必须 < 10 分钟**（连续两次 run 取第二次，避开冷缓存）。
- **job 实例数 ≤ 20**（GitHub Free 并发上限），macOS 实例 ≤ 5。目标结构：main 20 个、PR 18 个。
- **不删除检查、不降低断言强度、不缩小平台覆盖。** PR 仍执行三平台 `fmt` / `clippy`、Linux 全量 workspace 测试与各平台 Gate；Windows/macOS 全量 workspace 测试仍在 main 执行（分片后并集仍覆盖全部 workspace 包）。
- **只有 main 写缓存。** 写者恰好 6 个 shared-key：`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`；其余 job `save-if: false`。
- **每个 rust 矩阵缓存 key 对应一种固定 cargo 形态**：`rust` job 的每个分片，其最后一条构建型 cargo 调用必须是 `cargo test --workspace --all-targets --all-features`（Linux 分片）或该分片的 `cargo test ${{ matrix.packages }} --all-targets --all-features`（core/desktop 分片）；其后允许同形态的 `-- --list`（不改变构建单元）。`governance-static` 用自己的 key，不受此约束。
- **`injection-suite.mjs` 已 997 行，`check-scale.mjs` 上限 1000 行**（`.yml` 不在扫描扩展名内，`ci.yml` 不受该门禁约束）。任何新增行都会让规模门禁失败——注入分片计划必须放进新文件。
- **所有 shell 命令在仓库根目录执行**；提交信息用中文，PR 与 Issue 用中文。
- 每项变更先有 Issue、在独立分支实施、通过关联 Issue 的 PR 合并；OpenSpec 规划在 propose 阶段，实现只在 apply 阶段。

## 与设计文档的两处偏差（已评估，按此处执行）

1. **`core` 分片仍构建前端并跑 `cargo clippy --workspace`。** 设计 §5.2 说 core 分片不依赖前端产物。但 `cargo clippy --workspace` 会编译 `echo-app`（`generate_context!` 需要 `apps/desktop/dist`），若改成 `-p echo-core` 就把 macOS/Windows 的 clippy 收窄到 core，而这两个平台的 `#[cfg]` 代码主要在 shell 与播放器 crate 里——那是覆盖收缩，违反 Global Constraints。代价是两个 core 分片各多约 70s setup，仍低于关键路径。
2. **依赖审计的"每日定时"放进独立 workflow。** 设计 §7.2 要求 main + 每日定时跑在线公告扫描。把 `schedule` 加进 `ci.yml` 会让整条 20 实例流水线每天空跑一次，与压缩目标相反。改为新增 `.github/workflows/dependency-audit.yml`（仅 `schedule` + `workflow_dispatch`，1 个 job）。

## Review Focus

1. **PR 跳过、main 必须真实执行的检查**（desktop 分片）：用户会预期 main 上五个 rust 分片全部 executed 而不是 skipped。由 Task 1 对 `rust-desktop` job 级 `if` 的精确断言与 Task 7 的注入证明固定。
2. **缓存写者集合被悄悄扩大**（给 reader 加 `save-if: true`）：会让 10 GB 配额重新溢出，R1 复发且无任何测试变红。由 Task 3 的 writer 集合精确断言固定。
3. **场景命令归一化后证据不再出现在输出里**：561 条场景的证据必须仍能在合并后的 workspace 测试输出中观测到。由 Task 6 的 `run-scenario.mjs -- --automated` 全量跑通固定。
4. **注入分片出现空片或重叠**：空片会让某一片永远绿、重叠会让同一证明跑两次。由 Task 5 的 `selectEntries` 断言与「计划键集 = 套件 id 集」比对固定。
5. **依赖审计在线半在 PR 上被永久静默**：分层一旦写成常量 `1`，在线扫描就再也不会在任何事件上执行。由 Task 2 的 env 表达式精确断言 + 定时 workflow 存在性检查固定。

---

### Task 0: Issue、分支与 OpenSpec change（流程门）

**Files:**
- Create: `openspec/changes/ci-runtime-under-10-min/`（proposal.md、design.md、tasks.md、specs/engineering-governance/spec.md）

**Interfaces:**
- Produces: Issue 编号（后续 PR 必须引用）；分支 `feat/ci-runtime-under-10-min`；change 名 `ci-runtime-under-10-min`。

- [ ] **Step 1: 创建 GitHub Issue（中文）**

```bash
gh issue create --title "压缩 CI 运行耗时至 10 分钟内并降低失败率" --body "$(cat <<'EOF'
## 背景
最近一次 main run（43827801014）墙钟 23m47s，PR 平均约 17.5 分钟。

## 根因
- Actions 缓存 10.13 GB 超过 GitHub Free 的 10 GB 上限，三平台 Rust job 同一次 run 全部未命中缓存，Test 步骤冷编译 318–413 个 crate。
- 同一 job 内 clippy（check 形态）与 test（build 形态）互相作废。
- 单 job 串行堆叠多条关键路径；脚本内存在 `--locked` / `--lib` / 默认 feature 等异形 cargo 调用。

## 目标
- PR 与 main 的 CI 墙钟均 < 10 分钟。
- 降低失败率（缓存命中 + 依赖审计分层 + 注入分片）。
- 不删除检查、不放宽断言、不缩小平台覆盖。

设计与计划：docs/superpowers/specs/2026-11-16-ci-runtime-compression-design.md
EOF
)"
```

- [ ] **Step 2: 创建分支**

```bash
git switch -c feat/ci-runtime-under-10-min
```

- [ ] **Step 3: 运行 OpenSpec propose，生成 change**

用 `openspec-propose` 技能，change 名 `ci-runtime-under-10-min`，输入为已批准的设计文档。delta spec 必须覆盖 `engineering-governance` 的以下改动：
- 「CI 执行分层与结构完整性」：Windows/macOS 全量 workspace 测试 MAY 按包分片，分片并集 MUST 覆盖全部 workspace 包；`clippy` MUST 在三平台以 workspace 形态执行；会写缓存的 job 其最后一条 cargo 调用 MUST 与缓存形态一致；缓存写入 MUST 只发生在 `main`。
- 「门禁不得静默通过」：依赖审计的在线半 MAY 在 PR 上跳过，但 MUST 在 `main` 与定时任务上真实执行，且跳过时 MUST 打印显式 note；该分层 MUST 由结构门禁断言。
- `tasks.md` 的条目与下面 Task 1–8 一一对应（Task 9 是文档、归档与 PR，不进 tasks.md）。

- [ ] **Step 4: 提交**

```bash
git add openspec/changes/ci-runtime-under-10-min
git commit -m "docs(openspec): 提出 ci-runtime-under-10-min 变更"
```

---

### Task 1: rust 矩阵 5 分片、步骤重排、purity 迁出

**Files:**
- Modify: `.github/workflows/ci.yml:13-117`（`rust` job）
- Modify: `.github/workflows/ci.yml`（新增 `macos-native-gate` job）
- Modify: `scripts/verify/workflow-matrix.mjs`
- Test: `scripts/verify/checks/task-1.5.mjs`

**Interfaces:**
- Consumes: `parseWorkflow`、`matrixEntries`、`assertPlatformName`（已存在）。
- Produces: `assertShardMatrix(entries, expected, label)` —— 以 `` `${platform}/${shard}` `` 为键做逐字段 deep-equal，缺条目/多条目/字段漂移都以 `FAIL 1.5:` 前缀报错。Task 3 与 Task 5 会复用同一断言风格。

- [ ] **Step 1: 写失败的断言（`workflow-matrix.mjs`）**

在 `assertPlatformMatrix` 之后追加：

```js
export function assertShardMatrix(entries, expected, label) {
  const idOf = (entry) => `${entry.platform}/${entry.shard}`;
  const actual = new Map(entries.map((entry) => [idOf(entry), entry]));
  if (actual.size !== entries.length) {
    throw new Error(`${label} matrix contains duplicate platform/shard entries`);
  }
  for (const [id, mapping] of Object.entries(expected)) {
    const entry = actual.get(id);
    if (!entry) throw new Error(`${label} matrix is missing entry '${id}'`);
    for (const [field, value] of Object.entries(mapping)) {
      if (entry[field] !== value) {
        throw new Error(`${label} matrix entry '${id}' maps ${field} to '${entry[field]}', expected '${value}'`);
      }
    }
    const extra = Object.keys(entry).filter((field) => field !== "platform" && field !== "shard" && !(field in mapping));
    if (extra.length) throw new Error(`${label} matrix entry '${id}' has unexpected field(s): ${extra.join(", ")}`);
  }
  if (actual.size !== Object.keys(expected).length) {
    const unexpected = [...actual.keys()].filter((id) => !(id in expected));
    throw new Error(`${label} matrix contains unexpected entries: ${unexpected.join(", ")}`);
  }
}
```

- [ ] **Step 2: 写失败的断言（`task-1.5.mjs`）**

先把第 16 行的 import 改为：

```js
import { assertPlatformMatrix, assertPlatformName, assertShardMatrix, matrixEntries, parseWorkflow } from "../workflow-matrix.mjs";
```

（`assertPlatformMatrix` 仍被 `frontend` 矩阵使用，保留。）

把 `assertPlatformMatrix(matrixEntries(workflow, "rust", ...))` 换成（保持在原 try 块内）：

```js
const RUST_MATRIX = {
  "Linux/workspace":   { os: "ubuntu-latest",  cache: "linux-workspace",  packages: "--workspace",    clippy: true },
  "macOS/core":        { os: "xcode-27",       cache: "macos-core",       packages: "-p echo-core",   clippy: true },
  "Windows/core":      { os: "windows-latest", cache: "windows-core",     packages: "-p echo-core",   clippy: true },
};
assertShardMatrix(matrixEntries(workflow, "rust", "ci.yml"), RUST_MATRIX, "ci.yml rust");
assertPlatformName(workflow.jobs.rust.name, "Rust (${{ matrix.platform }} ${{ matrix.shard }})", "ci.yml rust name");

const RUST_DESKTOP_MATRIX = {
  "macOS/desktop":     { os: "xcode-27",       cache: "macos-desktop",    packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false },
  "Windows/desktop":   { os: "windows-latest", cache: "windows-desktop",  packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false },
};
assertShardMatrix(matrixEntries(workflow, "rust-desktop", "ci.yml"), RUST_DESKTOP_MATRIX, "ci.yml rust-desktop");
assertPlatformName(workflow.jobs["rust-desktop"].name, "Rust desktop (${{ matrix.platform }})", "ci.yml rust-desktop name");
```

**为什么拆成两个 job（不要合并回一个矩阵）**：GitHub 的 `jobs.<job_id>.if` 在矩阵展开**之前**求值，其可用上下文只有 `github` / `needs` / `vars` / `inputs`——`matrix` 不在其中，写 `matrix.shard != 'desktop'` 会让整个 workflow 在解析期被拒（所有 job 一起消失，等于删光全部检查）。因此「PR 上不存在 desktop 分片」只能靠 job 级 `if` 表达，而 job 级 `if` 看不到矩阵维度：分片必须落在不同 job 里。

追加一条永久断言，禁止任何 job 级 `if` 引用矩阵上下文（这条挡住整类缺陷，而不是只挡住本文件当前写法）。用正则而不是 `includes("matrix.")`：后者漏掉 `matrix['shard']` 这种下标写法，而 GitHub 同样会拒绝它：

```js
const MATRIX_CONTEXT = /(^|[^A-Za-z0-9_-])matrix\s*[.[]/;
for (const [jobId, job] of Object.entries(workflow.jobs)) {
  if (MATRIX_CONTEXT.test(String(job.if || ""))) {
    fail(`ci.yml job '${jobId}' if must not reference the matrix context (job-level if is evaluated before the matrix expands)`);
  }
}
```

同时做三处删除/替换，否则模板化后必然失败：

1. **`REQUIRED` 数组（第 96–112 行）删掉 `"cargo test --workspace --all-targets --all-features"` 一行。** 该数组用 `ci.includes(needle)` 做字面量匹配，模板化后这个字面量不再出现；其形态由下面的 `test.run` 精确断言覆盖（更强）。`cargo fmt --all -- --check` 与 `cargo clippy --workspace ...` 两行保持不动。
2. **整段删除第 118–153 行**（从 `const test = step(...)` 到 `Task 9.7 9.8` 的 `run` 断言），然后用下面的块替代。唯一要原样保留的是 `const format = step(workflow, "rust", "Format check")` 与它的 `format.run` 断言——下面的块不含它们，替代时把这两行放回块首。
3. `assertPlatformName` 那行在第 90 行，单独替换（见上）。

替换 `rust` 的步骤断言：

```js
const test = step(workflow, "rust", "Test");
const clippy = step(workflow, "rust", "Clippy");
const rustStatic = step(workflow, "rust", "Rust static gates");
const TEST_IF = "github.event_name != 'pull_request' || matrix.shard == 'workspace'";
if (String(test.if || "").trim() !== TEST_IF) fail(`rust Test if must be '${TEST_IF}'`);
if (test["continue-on-error"] === true) fail("rust Test must not set continue-on-error");
if (test.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
if (String(clippy.if || "").trim() !== "matrix.clippy") fail("rust Clippy if must be 'matrix.clippy'");
if (clippy.run !== "cargo clippy --workspace --all-targets --all-features -- -D warnings") {
  fail("rust Clippy run must be 'cargo clippy --workspace --all-targets --all-features -- -D warnings'");
}
for (const stepName of ["Task 16.1", "Task 9.7 9.8", "Rust static gates"]) {
  const s = step(workflow, "rust", stepName);
  if (String(s.if || "").trim() !== "matrix.shard == 'workspace'") {
    fail(`rust ${stepName} if must be "matrix.shard == 'workspace'"`);
  }
}
if (rustStatic.run !== "pnpm verify:governance -- --sections=rust-static") {
  fail("Rust static gates run must be 'pnpm verify:governance -- --sections=rust-static'");
}
if (rustStatic.env?.ECHO_PURITY_REQUIRE_BUILD !== undefined) {
  fail("Rust static gates must not set ECHO_PURITY_REQUIRE_BUILD (purity moved to governance-static)");
}

// rust-desktop: the shards that carry the rest of the workspace tests. They are
// a separate job because a job-level `if` cannot see matrix.*, and this job must
// be absent on pull requests while running in full on main.
const DESKTOP_JOB_IF = "github.event_name != 'pull_request'";
if (String(workflow.jobs["rust-desktop"].if || "").trim() !== DESKTOP_JOB_IF) {
  fail(`rust-desktop job if must be '${DESKTOP_JOB_IF}'`);
}
const desktopTest = step(workflow, "rust-desktop", "Test");
if (String(desktopTest.if || "").trim() !== "") fail("rust-desktop Test must not be conditionally skipped");
if (desktopTest["continue-on-error"] === true) fail("rust-desktop Test must not set continue-on-error");
if (desktopTest.run !== "cargo test ${{ matrix.packages }} --all-targets --all-features") {
  fail("rust-desktop Test run must be 'cargo test ${{ matrix.packages }} --all-targets --all-features'");
}
```

追加步骤顺序断言（这是缓存形态稳定性的结构保证）：

```js
const RUST_ORDER = ["Format check", "Task 16.1", "Task 9.7 9.8", "Clippy", "Test", "Rust static gates"];
const rustSteps = workflow.jobs.rust.steps.map((s) => s.name).filter(Boolean);
const positions = RUST_ORDER.map((name) => rustSteps.indexOf(name));
if (!positions.every((v, i) => v >= 0 && (i === 0 || v > positions[i - 1]))) {
  fail(`rust step order must be ${RUST_ORDER.join(" < ")}`);
}
const DESKTOP_ORDER = ["Format check", "Test"];
const desktopSteps = workflow.jobs["rust-desktop"].steps.map((s) => s.name).filter(Boolean);
const desktopPositions = DESKTOP_ORDER.map((name) => desktopSteps.indexOf(name));
if (!desktopPositions.every((v, i) => v >= 0 && (i === 0 || v > desktopPositions[i - 1]))) {
  fail(`rust-desktop step order must be ${DESKTOP_ORDER.join(" < ")}`);
}
```

把 `purity` 的 section 映射改到 governance-static，并追加该步骤的精确断言：

```js
// SECTION_SELECTOR_STEP 中：
purity: ["governance-static", "Run build purity gate"],
// 追加：
const purityStep = step(workflow, "governance-static", "Run build purity gate");
if (purityStep.run !== "pnpm verify:governance -- --sections=purity") {
  fail("governance-static purity step run must be 'pnpm verify:governance -- --sections=purity'");
}
assertEnv(purityStep, "ECHO_PURITY_REQUIRE_BUILD", "1", "governance-static purity step");
```

把 `rust` 从 `assertAlwaysRuns` 列表移除（`rust-desktop` 有意在 PR 上缺席，同样不进列表），并把平台 Gate 与 frontend 加进列表：

```js
for (const jobId of ["frontend", "governance-static", "injection-proofs", "release-gate", "perf", "task-gates", "macos-platform-gate", "macos-native-gate", "windows-platform-gate"]) {
  assertAlwaysRuns(workflow, jobId);
}
```

`rust` 与 `rust-desktop` 都不能进这个列表：`rust` 的 `Test` 步骤按分片 `if`，`rust-desktop` 整 job 在 PR 上缺席——它们的执行条件由上面各自的精确断言固定。

- [ ] **Step 3: 运行，确认失败**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: ci.yml rust matrix is missing entry 'Linux/workspace'`

- [ ] **Step 4: 重写 `ci.yml` 的 `rust` job**

矩阵（无 job 级 `if`——`matrix` 在 job 级 `if` 里不可用）：

```yaml
  rust:
    name: Rust (${{ matrix.platform }} ${{ matrix.shard }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - { platform: Linux,   os: ubuntu-latest,  shard: workspace, cache: linux-workspace,  packages: "--workspace",   clippy: true }
          - { platform: macOS,   os: xcode-27,       shard: core,      cache: macos-core,       packages: "-p echo-core",  clippy: true }
          - { platform: Windows, os: windows-latest, shard: core,      cache: windows-core,     packages: "-p echo-core",  clippy: true }

  # The macOS/Windows `desktop` shards carry the rest of the workspace tests.
  # They must be absent on pull requests and fully present on main, which a
  # job-level `if` can express only because it no longer needs matrix.* (GitHub
  # evaluates a job-level `if` before the matrix expands, where `matrix` is not
  # an available context).
  rust-desktop:
    name: Rust desktop (${{ matrix.platform }})
    runs-on: ${{ matrix.os }}
    if: github.event_name != 'pull_request'
    strategy:
      fail-fast: false
      matrix:
        include:
          - { platform: macOS,   os: xcode-27,       shard: desktop,   cache: macos-desktop,   packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false }
          - { platform: Windows, os: windows-latest, shard: desktop,   cache: windows-desktop, packages: "-p echo-sync -p echo-desktop -p echo-app", clippy: false }
```

步骤顺序固定为 `Format check → Task 16.1 → Task 9.7 9.8 → Clippy → Test → Rust static gates`。`Task 16.1`、`Task 9.7 9.8`、`Rust static gates` 的 `if` 改为 `matrix.shard == 'workspace'`；`Rust static gates` 的 `run` 改为 `pnpm verify:governance -- --sections=rust-static` 并删除 `ECHO_PURITY_REQUIRE_BUILD`；`Clippy` 加 `if: matrix.clippy`；`Test` 的 `if` 改为 `github.event_name != 'pull_request' || matrix.shard == 'workspace'`、`run` 改为 `cargo test ${{ matrix.packages }} --all-targets --all-features`。删除原 `Verify libmpv smoke and native e2e gates` 步骤（迁到新 job）。

`rust-desktop` 是 `rust` job 的副本，按上面矩阵收缩后只保留必需步骤：`checkout`（同样带 `lfs`）、Windows Developer Mode、toolchain、cache、Linux webview（该矩阵无 Linux 条目，可省）、`Format check`、node/pnpm/install、`Build frontend for Tauri context`、`Test`（**无 `if`**，`run` 与 `rust` 的 `Test` 完全一致）。不要复制 `Task 16.1` / `Task 9.7 9.8` / `Rust static gates`（它们只在 Linux workspace 分片有意义），也不要复制 `Clippy`（两个 desktop 分片 `clippy: false`，且 clippy 已由 core 分片以 workspace 形态在三平台执行）。

`checkout`、Developer Mode、toolchain、cache、webkit、node/pnpm、install、frontend build 保持原样（cache 的 `shared-key` / `save-if` 留给 Task 3）。

- [ ] **Step 5: 新增 `macos-native-gate` job（插在 `macos-platform-gate` 之后）**

```yaml
  macos-native-gate:
    name: macOS native Gate
    runs-on: xcode-27
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain (pinned)
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1

      - name: Cache Rust build artifacts
        uses: Swatinem/rust-cache@v2

      # 8.12/13.2 load the vendored dylibs through DYLD_LIBRARY_PATH at
      # target/Frameworks, which apps/desktop/src-tauri/build.rs stages when the
      # shell is built. This job never builds the shell, so it stages the same
      # vendored set itself (build.rs selects by the `.dylib` extension, and the
      # vendor dir holds only regular files); 8.12 fails loudly if it is empty.
      - name: Stage vendored libmpv dylibs
        run: |
          mkdir -p target/Frameworks
          cp apps/desktop/src-tauri/vendor/libmpv/macos/*.dylib target/Frameworks/

      - uses: actions/setup-node@v4
        with:
          node-version-file: .nvmrc

      - name: Install pnpm
        uses: pnpm/action-setup@v4
        with:
          version: 11.18.0

      - name: Verify libmpv smoke and native e2e gates
        run: pnpm verify:task -- 8.12 13.2
```

不执行 `pnpm install`：`run-task.mjs` 只用 Node 内置模块。

- [ ] **Step 6: 在 `governance-static` 中新增 purity 步骤（放在 `Run governance static gates` 之前）**

```yaml
      # purity used to run at the end of the Rust Linux leg, which left a
      # default-feature `cargo check` as that job's last cargo call and broke
      # the cache-shape invariant. It needs only cargo and this job's target
      # dir, so it lives here, before coverage.
      - name: Run build purity gate
        env:
          ECHO_PURITY_REQUIRE_BUILD: "1"
        run: pnpm verify:governance -- --sections=purity
```

- [ ] **Step 7: 运行，确认通过**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `ok 1.5: three-platform CI matrix declares ...`

Run: `node -e "import('./scripts/verify/workflow-matrix.mjs').then(m=>console.log(typeof m.assertShardMatrix))"`
Expected: `function`

- [ ] **Step 8: 提交**

```bash
git add .github/workflows/ci.yml scripts/verify/workflow-matrix.mjs scripts/verify/checks/task-1.5.mjs
git commit -m "ci: rust 矩阵拆为 5 个分片并把 purity 迁到 governance-static"
```

---

### Task 2: 依赖审计分层、删除 dependency-audit job、新增定时 workflow

**Files:**
- Modify: `scripts/verify/checks/task-1.7.mjs`
- Modify: `scripts/verify/run-task.mjs`（成功路径透传 stderr）
- Modify: `.github/workflows/ci.yml:233-266`（删除 `dependency-audit`，在 `governance-static` 增加 1.7 步骤）
- Create: `.github/workflows/dependency-audit.yml`
- Test: `scripts/verify/checks/task-1.5.mjs`

**Interfaces:**
- Produces: 环境变量 `ECHO_AUDIT_OFFLINE_ONLY=1` —— `task-1.7` 在离线半（`cargo deny check licenses|bans|sources` + `LICENSES.md`）之后打印显式 note 并跳过在线半。

- [ ] **Step 1: 在 `task-1.7.mjs` 中加离线开关**

在 `const AUDIT_IGNORES = ...` 之前插入：

```js
// Tiering: on pull requests the online half (advisory DB, registry audit) is
// skipped — a new RUSTSEC advisory has nothing to do with the PR under review
// and would block every in-flight PR at once. It still runs for real on pushes
// to main and in the scheduled Dependency audit workflow. The skip is never
// silent: the note below names what did not run.
const OFFLINE_ONLY = process.env.ECHO_AUDIT_OFFLINE_ONLY === "1";
```

把第 3 节（`handleOnline` 的两条 + `cargo audit` 块）与第 4 节（`auditPnpm()` 调用块）整体包进 `if (!OFFLINE_ONLY) { ... }`，并在 `else` 分支打印：

```js
if (OFFLINE_ONLY) {
  process.stderr.write(
    "1.7: note — online advisory checks (cargo deny advisories, cargo audit, pnpm audit --prod) " +
      "skipped on this event; they run for real on pushes to main and in the scheduled Dependency audit workflow\n",
  );
}
```

- [ ] **Step 2: 运行，确认离线开关生效**

Run: `ECHO_AUDIT_OFFLINE_ONLY=1 node scripts/verify/checks/task-1.7.mjs`
Expected: 退出 0；stderr 含 `1.7: note — online advisory checks`；输出中不出现在线半的三段（`cargo deny check` 的 advisories 段、`cargo audit` 的漏洞列表、`pnpm audit` 的汇总）。离线半（licenses/bans/sources、ignore 镜像、LICENSES.md）仍照常执行。

- [ ] **Step 3: 在 `task-1.5.mjs` 中加断言**

```js
const auditStep = step(workflow, "governance-static", "Verify dependency audit gate");
if (auditStep.run !== "pnpm verify:task -- 1.7") fail("governance-static audit step run must be 'pnpm verify:task -- 1.7'");
const AUDIT_ENV = "${{ github.event_name == 'pull_request' && '1' || '0' }}";
if (String(auditStep.env?.ECHO_AUDIT_OFFLINE_ONLY) !== AUDIT_ENV) {
  fail(`governance-static audit step env ECHO_AUDIT_OFFLINE_ONLY must be '${AUDIT_ENV}'`);
}
if (workflow.jobs["dependency-audit"]) fail("ci.yml job 'dependency-audit' must not exist (folded into governance-static)");
// A step-level `if` would keep every string above intact while the scan stops
// running — exactly the failure this tiering exists to prevent.
if (String(auditStep.if || "").trim() !== "") fail("governance-static audit step must not be conditionally skipped");
// The skip must never be silent: the gate itself carries the note naming what
// did not run, and this pins its presence so a later edit cannot drop it.
const auditSource = readFileSync(resolve(ROOT, "scripts", "verify", "checks", "task-1.7.mjs"), "utf8");
for (const needle of ["ECHO_AUDIT_OFFLINE_ONLY", "1.7: note — online advisory checks"]) {
  if (!auditSource.includes(needle)) fail(`task-1.7 must keep '${needle}'`);
}
// The scheduled workflow is the only place the online half runs on a quiet day,
// so it must stay schedule/dispatch-only: a `pull_request` trigger would re-run
// it on every PR and defeat the point of taking it out of ci.yml.
const auditWorkflow = parseWorkflow(
  readFileSync(resolve(ROOT, ".github", "workflows", "dependency-audit.yml"), "utf8"),
  "dependency-audit.yml",
);
const auditTriggers = Object.keys(auditWorkflow.on || {}).sort();
if (JSON.stringify(auditTriggers) !== JSON.stringify(["schedule", "workflow_dispatch"])) {
  fail(`dependency-audit.yml must trigger on exactly schedule + workflow_dispatch, got [${auditTriggers.join(", ")}]`);
}
```

- [ ] **Step 3b: 让 runner 在成功路径也透传子进程 stderr**

CI 跑的是 `pnpm verify:task -- 1.7`，而 `run-task.mjs` 用 `stdio: ["ignore","pipe","pipe"]` 捕获输出、成功时直接返回——上面那条 note 因此永远进不了 CI 日志，规格的「MUST NOT 静默」在真实流水线里落空。把 `runCommand` 改成：成功时把子进程 stderr 原样转发，失败时保持现有诊断行为。

```js
function runCommand(cmd, taskId) {
  const result = spawnSync(cmd, {
    cwd: ROOT,
    shell: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    // Surface the task's own diagnostic output — swallowing it makes a
    // failing gate (e.g. macOS task-1.9) impossible to debug from CI logs.
    const stderr = `${result.stderr || ""}`.trim();
    const stdout = `${result.stdout || ""}`.trim();
    const detail = stderr || stdout;
    fail(
      `task ${taskId}: command exited ${result.status}: ${cmd}` +
        (detail ? `\n${detail}` : ""),
    );
    return false;
  }
  // A passing task may still have something the log must carry — task 1.7's
  // skip note says which advisory checks did NOT run on this event, and a
  // silent skip is the failure mode the tiering exists to avoid.
  if (result.stderr) process.stderr.write(result.stderr);
  return true;
}
```

- [ ] **Step 3c: PR 上不再安装 cargo-audit**

`cargo-audit` 只在在线半被调用。把 `task-1.7.mjs` 里的 `ensureTool("cargo-audit");` 改为：

```js
if (!OFFLINE_ONLY) ensureTool("cargo-audit");
```

（`ensureTool("cargo-deny")` 保持无条件——离线半的 `cargo deny check licenses|bans|sources` 需要它。）

- [ ] **Step 4: 运行，确认失败**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: ci.yml job 'governance-static' is missing step 'Verify dependency audit gate'`

- [ ] **Step 5: 改 `ci.yml`**

删除整个 `dependency-audit` job。在 `governance-static` 的 `Run governance static gates` 之后追加：

```yaml
      # task-1.7 installs missing tools itself, but naming the install step keeps
      # the multi-minute compile out of the gate's own log and fails early.
      # cargo-audit is only used by the online half, so PRs skip it entirely.
      - name: Install audit tooling
        run: |
          cargo install cargo-deny --locked
          if [ "$ECHO_AUDIT_OFFLINE_ONLY" != "1" ]; then cargo install cargo-audit --locked; fi
        env:
          ECHO_AUDIT_OFFLINE_ONLY: ${{ github.event_name == 'pull_request' && '1' || '0' }}

      # The offline-safe half (deny.toml licenses/bans/sources, the
      # advisory-ignore mirror and LICENSES.md) always gates. The online half
      # runs on main and in the scheduled Dependency audit workflow; on PRs it
      # is skipped with an explicit note (a new advisory must not block every
      # in-flight PR).
      - name: Verify dependency audit gate
        env:
          ECHO_AUDIT_OFFLINE_ONLY: ${{ github.event_name == 'pull_request' && '1' || '0' }}
        run: pnpm verify:task -- 1.7
```

同时把 `task-gates` 里提到 `1.7 runs in the dependency-audit job` 的注释改成 `1.7 runs in the governance-static job`。

- [ ] **Step 6: 新建 `.github/workflows/dependency-audit.yml`**

```yaml
name: Dependency audit

# The online advisory scan must keep running even on days with no push to main;
# it lives in its own workflow so the 20-instance CI pipeline is not re-run daily.
on:
  schedule:
    - cron: "0 3 * * *"
  workflow_dispatch:

jobs:
  audit:
    name: Dependency audit (online)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain (pinned)
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1

      - name: Cache Rust build artifacts
        uses: Swatinem/rust-cache@v2
        with:
          shared-key: governance-static
          save-if: false

      - uses: actions/setup-node@v4
        with:
          node-version-file: .nvmrc

      - name: Install pnpm
        uses: pnpm/action-setup@v4
        with:
          version: 11.18.0

      - name: Install dependencies
        run: pnpm install --frozen-lockfile

      - name: Install audit tooling
        run: cargo install cargo-deny cargo-audit --locked

      # No ECHO_AUDIT_OFFLINE_ONLY here on purpose: the scheduled run is the one
      # place the online half executes on a quiet day, so it must never take the
      # skip branch.
      - name: Verify dependency audit gate
        run: pnpm verify:task -- 1.7
```

- [ ] **Step 7: 运行，确认通过**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `ok 1.5: ...`

Run: `node -e "const y=require('js-yaml'),fs=require('fs');const w=y.load(fs.readFileSync('.github/workflows/dependency-audit.yml','utf8'));if(!w.on?.schedule)throw new Error('missing schedule');const runs=Object.values(w.jobs).flatMap(j=>j.steps||[]).map(s=>s.run||'');if(!runs.includes('pnpm verify:task -- 1.7'))throw new Error('audit step missing');console.log('yaml ok')"`
Expected: `yaml ok`（同时验证了 `on.schedule` 与 1.7 步骤存在）。

- [ ] **Step 8: 提交**

```bash
git add .github/workflows/ci.yml .github/workflows/dependency-audit.yml scripts/verify/checks/task-1.5.mjs scripts/verify/checks/task-1.7.mjs scripts/verify/run-task.mjs
git commit -m "ci: 依赖审计分层，在线半迁到 main 与定时 workflow"
```

---

### Task 3: 缓存治理（shared-key + save-if）

**Files:**
- Modify: `.github/workflows/ci.yml`（全部 `Cache Rust build artifacts` 步骤；顺带修正 `task-gates` 注释里 1.7 安装工具的描述）
- Test: `scripts/verify/checks/task-1.5.mjs`

**Interfaces:**
- Consumes: `assertShardMatrix` 建立的矩阵字段 `cache`（Task 1）。
- Produces: 缓存写者集合断言 `CACHE_WRITERS`，Task 7 的注入证明会复用它。

- [ ] **Step 1: 在 `task-1.5.mjs` 中加断言**

```js
// 用块作用域包住：Task 2 的审计触发断言已在模块作用域声明 auditWorkflow，
// 这里同名常量若留在模块作用域会直接抛 "Identifier has already been declared"。
{
const CACHE_STEP = "Cache Rust build artifacts";
const MAIN_ONLY = "${{ github.ref == 'refs/heads/main' }}";
const CACHE_WRITERS = ["governance-static", "linux-workspace", "macos-core", "macos-desktop", "windows-core", "windows-desktop"];
// rust/rust-desktop 的 shared-key 是模板表达式，必须各自展开成该 job 实际写入
// 的 key。按 job 展开而不是合并两个矩阵：两个 job 都写入时合并会对同一批 key
// 重复计数（实测会得到 11 项而不是 6 项）。
const writerKeys = [];
const readerKeys = [];
for (const [jobId, job] of Object.entries(workflow.jobs)) {
  const cacheStep = (job.steps || []).find((s) => s.name === CACHE_STEP);
  if (!cacheStep) continue;
  const sharedKey = String(cacheStep.with?.["shared-key"] || "");
  const saveIf = String(cacheStep.with?.["save-if"] ?? "");
  if (!sharedKey) fail(`ci.yml job '${jobId}' ${CACHE_STEP} must set shared-key`);
  if (saveIf !== MAIN_ONLY && saveIf !== "false") {
    fail(`ci.yml job '${jobId}' ${CACHE_STEP} save-if must be 'false' or '${MAIN_ONLY}', got '${saveIf}'`);
  }
  if (saveIf === MAIN_ONLY) {
    writerKeys.push(...(sharedKey === "${{ matrix.cache }}" ? matrixEntries(workflow, jobId, "ci.yml").map((entry) => entry.cache) : [sharedKey]));
  } else {
    readerKeys.push(sharedKey);
  }
}
if (JSON.stringify(writerKeys.sort()) !== JSON.stringify(CACHE_WRITERS)) {
  fail(`ci.yml cache writers must be exactly [${CACHE_WRITERS.join(", ")}], got [${writerKeys.join(", ")}]`);
}
for (const key of readerKeys) {
  if (key !== "${{ matrix.cache }}" && !CACHE_WRITERS.includes(key)) {
    fail(`ci.yml reader cache key '${key}' has no writer`);
  }
}
// governance-static's key is job-private: the spec exempts it from the cargo
// shape invariant *because* nobody else builds into it. Pin that readership so
// the exemption cannot silently become false.
const governanceReaders = Object.entries(workflow.jobs)
  .filter(([, job]) => (job.steps || []).some((s) => s.name === CACHE_STEP && String(s.with?.["shared-key"] || "") === "governance-static"))
  .map(([jobId]) => jobId);
if (JSON.stringify(governanceReaders) !== JSON.stringify(["governance-static"])) {
  fail(`ci.yml cache key 'governance-static' must be read only by job 'governance-static', got [${governanceReaders.join(", ")}]`);
}
// ...and the scheduled audit workflow is its only other reader, so it must
// restore the same key rather than compiling its own cold tree every night.
const auditWorkflow = parseWorkflow(
  readFileSync(resolve(ROOT, ".github", "workflows", "dependency-audit.yml"), "utf8"),
  "dependency-audit.yml",
);
const auditKeys = Object.values(auditWorkflow.jobs).flatMap((job) =>
  (job.steps || []).filter((s) => s.name === CACHE_STEP).map((s) => String(s.with?.["shared-key"] || "")),
);
if (auditKeys.length !== 1 || auditKeys[0] !== "governance-static") {
  fail(`dependency-audit.yml ${CACHE_STEP} must use shared-key 'governance-static', got [${auditKeys.join(", ")}]`);
}
// The scheduled workflow is the online half's one guaranteed execution point on
// a quiet day. Deleting the step that runs 1.7 would otherwise be silent: the
// triggers stay right, the cache key stays right, and advisories simply stop
// being scanned.
const auditRunSteps = Object.values(auditWorkflow.jobs).flatMap((job) =>
  (job.steps || []).map((s) => String(s.run || "")),
);
if (!auditRunSteps.some((run) => run.includes("verify:task -- 1.7"))) {
  fail("dependency-audit.yml must actually run 'pnpm verify:task -- 1.7'");
}
}
```

`parseWorkflow`、`readFileSync`、`resolve` 与 `ROOT` 均已在 `task-1.5.mjs` 顶部就绪，无需新增 import。

- [ ] **Step 2: 运行，确认失败**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: ci.yml job 'rust' Cache Rust build artifacts must set shared-key`

- [ ] **Step 3: 给每个缓存步骤补 `shared-key` / `save-if`**

| job | `shared-key` | `save-if` |
|---|---|---|
| `rust` | `${{ matrix.cache }}` | `${{ github.ref == 'refs/heads/main' }}` |
| `rust-desktop` | `${{ matrix.cache }}` | `${{ github.ref == 'refs/heads/main' }}` |
| `task-gates` | `linux-workspace` | `false` |
| `governance-static` | `governance-static` | `${{ github.ref == 'refs/heads/main' }}` |
| `macos-platform-gate` | `macos-desktop` | `false` |
| `macos-native-gate` | `macos-desktop` | `false` |
| `windows-platform-gate` | `windows-desktop` | `false` |
| `injection-proofs` | `linux-workspace` | `false` |
| `release-gate` | `linux-workspace` | `false` |
| `perf` | `linux-workspace` | `false` |

每个步骤形如：

```yaml
      - name: Cache Rust build artifacts
        uses: Swatinem/rust-cache@v2
        with:
          shared-key: linux-workspace
          save-if: false
```

同时修正 `task-gates` 里关于 1.7 的注释（`ci.yml` 约 267-268 行）：把
`1.7 runs in the governance-static job (it installs cargo-deny/cargo-audit)`
改为 `1.7 runs in the governance-static job (it installs cargo-deny, plus cargo-audit on non-PR events)`
—— PR 上已不安装 cargo-audit，原注释与实现不符。

- [ ] **Step 4: 运行，确认通过**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `ok 1.5: ...`

- [ ] **Step 4b: 自测新断言非恒真**

临时把 `.github/workflows/dependency-audit.yml` 的 `Verify dependency audit gate` 步骤名下的
`run: pnpm verify:task -- 1.7` 改成 `run: pnpm verify:task -- 1.5`，
Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: dependency-audit.yml must actually run 'pnpm verify:task -- 1.7'`
改回后再次运行恢复 `ok 1.5: ...`。

- [ ] **Step 5: 删除现存旧缓存（一次性运维步骤）**

```bash
gh cache list --limit 100 --json id,key,sizeInBytes,ref
gh cache delete --all
gh api repos/zsxink/Echo/actions/cache/usage
```

Expected: 删除后 `active_caches_count` 为 0（或仅剩本次分支产生的新 key）。

- [ ] **Step 6: 提交**

```bash
git add .github/workflows/ci.yml scripts/verify/checks/task-1.5.mjs
git commit -m "ci: 缓存按 shared-key 收敛，只有 main 写入"
```

---

### Task 4: task-gates 4 → 3 分片

**Files:**
- Modify: `.github/workflows/ci.yml:162-176`（`task-gates` 矩阵）
- Test: `scripts/verify/checks/task-1.5.mjs`

**Interfaces:**
- Produces: `EXPECTED_SHARDS` 三个键 `core` / `desktop` / `frontend`，并集仍为 42 项且含 `16.1` 的排除断言。

- [ ] **Step 1: 改断言**

```js
const EXPECTED_SHARDS = {
  core: "1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3".split(" "),
  desktop: "7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2".split(" "),
  frontend: "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1 10.7 10.8 10.9 10.10 12.1 12.2 12.3".split(" "),
};
```

`shards.length !== 4` 改为 `!== 3`，消息改为 `task-gates must have 3 shards, got ${shards.length}`。顶部注释「partitioned across four shards」改为 three。

- [ ] **Step 2: 运行，确认失败**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: task-gates must have 3 shards, got 4`

- [ ] **Step 3: 合并 `ci.yml` 的两个 frontend 分片**

把 `frontend-a` 与 `frontend-b` 两条替换为：

```yaml
          - shard: frontend
            tasks: "10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1 10.7 10.8 10.9 10.10 12.1 12.2 12.3"
```

- [ ] **Step 4: 运行，确认通过**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `ok 1.5: ...`

- [ ] **Step 5: 提交**

```bash
git add .github/workflows/ci.yml scripts/verify/checks/task-1.5.mjs
git commit -m "ci: task-gates 合并为 3 个分片"
```

---

### Task 5: 注入证明 2 → 3 分片

**Files:**
- Create: `scripts/verify/injection-shard-plan.mjs`
- Modify: `scripts/verify/injection-shard.mjs`
- Modify: `scripts/verify/injection-suite.mjs`（仅删除 `embedded-frontend/stale-binary` 上失效的 `shard: 1`）
- Modify: `.github/workflows/ci.yml:408-460`（`injection-proofs`）
- Test: `scripts/verify/checks/task-1.5.mjs`

**Interfaces:**
- Produces: `ENTRY_SHARDS` —— 以注入条目 id 为键、分片号为值的对象，初始覆盖全部 45 条（Task 7 会再加 7 条）；`selectEntries` 在 `shard` 非空时要求每个条目都在表中，否则抛 `entry '<id>' has no shard in injection-shard-plan.mjs`。

- [ ] **Step 1: 取全部条目 id**

Run: `node scripts/verify/injection-suite.mjs --list`
Expected: 45 行 id。

- [ ] **Step 2: 新建 `scripts/verify/injection-shard-plan.mjs`**

```js
// Explicit shard assignment for the injection proofs.
//
// `injection-suite.mjs` sits at 997 lines against the 1000-line scale ceiling,
// so the plan lives here instead of as a `shard:` field on every entry.
// Assignment is by measured cost, never by position: shard 1 also builds the
// shell binary for the embedded-frontend proof (~200s), so it carries only the
// cheap node-only entries; the cargo-heavy proofs are split across 2 and 3.
export const ENTRY_SHARDS = {
  // shard 1 — embedded frontend + node-only proofs
  "embedded-frontend/stale-binary": 1,
  "ci-platform-matrix/missing-linux-entry": 1,
  // ... 其余 node-only 条目
  // shard 2 — cargo-heavy proofs
  "file-open-normalization/raw-url-payload": 2,
  "scenario-command-proof/undeclared-hybrid": 2,
  "build-purity/ungated-test-only-module": 2,
  "native-attestation/missing-record": 2,
  "native-attestation/empty-record": 2,
  "native-attestation/incomplete-record": 2,
  "native-attestation/complete-record (positive control)": 2,
  // shard 3 — cargo list / workspace-test proofs
  "scenario-evidence/missing-target": 3,
  "scenario-evidence/unobserved-target": 3,
  "delegation/cargo-exit-status": 3,
  "delegation/pnpm-exit-status": 3,
  "delegation/runner-propagates-check-failure": 3,
  // ... 其余 node-only 条目
};
```

分配规则：`embedded-frontend/stale-binary`、全部 `ci-structure/*`、`ci-platform-matrix/*`、`release-platform-matrix/*`、`licensing/*`、`windows-installer/*`、`audio-effects-*`、`wire-dialogs/*`、`object-kind-layout/*`、`lint-inheritance/*`、`scale/*`、`toolchain/*`、`scenario-registry/*`、`verification-validity/*` 归 shard 1；`file-open-normalization/*`、`scenario-command-proof/*`、`build-purity/*`、`native-attestation/*` 归 shard 2；`scenario-evidence/*`、`delegation/*` 归 shard 3。写完后三片条目数必须都非空。

键必须是 `--list` 打印的**完整 id 字符串**，逐字复制——例如 `native-attestation/complete-record (positive control)` 带空格与括号后缀，漏掉后缀会让该条目无分片、`selectEntries` 直接抛错。按前缀规则应得到 33 / 7 / 5。

- [ ] **Step 3: 让 `injection-shard.mjs` 使用该表**

```js
import { ENTRY_SHARDS } from "./injection-shard-plan.mjs";
```

把 `const assigned = entry.shard ?? (position % total) + 1;` 改为：

```js
const assigned = ENTRY_SHARDS[entry.id] ?? entry.shard;
if (assigned === undefined) {
  throw new Error(`entry '${entry.id}' has no shard in injection-shard-plan.mjs`);
}
```

`position` 参数随之不再使用，删除 `filtered.forEach((entry, position)` 的 `position`。

同时删除 `injection-suite.mjs` 中 `embedded-frontend/stale-binary` 条目上的 `shard: 1`（约 585 行）——分片表现在是唯一权威，留着这个字段会被静默忽略，将来有人改它却不生效。

- [ ] **Step 4: 改 `task-1.5.mjs`**

```js
if (JSON.stringify(injection?.strategy?.matrix?.shard) !== JSON.stringify([1, 2, 3])) {
  fail("injection-proofs matrix.shard must be [1, 2, 3]");
}
assertRunIncludes(shardStep, "--shard=${{ matrix.shard }}/3", "injection shard step");
```

- [ ] **Step 5: 运行，确认失败**

Run: `node scripts/verify/checks/task-1.5.mjs`
Expected: `FAIL 1.5: injection-proofs matrix.shard must be [1, 2, 3]`

- [ ] **Step 6: 改 `ci.yml`**

`name: Injection proof ${{ matrix.shard }}/2` → `/3`；`shard: [1, 2]` → `shard: [1, 2, 3]`；`--shard=${{ matrix.shard }}/2` → `/3`。`matrix.shard == 1` 的两处保持不变。

- [ ] **Step 7: 运行，确认分片计划完整且三片非空**

注意：`--list` 在分片过滤**之前**短路（`injection-suite.mjs:763-766`），所以不能用 `--shard=$i/3 --list` 数条目。改为直接比对计划文件的键集与套件的 id 集：

```bash
node scripts/verify/injection-suite.mjs --list | sort > /tmp/injection-ids.txt
node -e "import('./scripts/verify/injection-shard-plan.mjs').then(({ENTRY_SHARDS}) => {
  const counts = {};
  for (const v of Object.values(ENTRY_SHARDS)) counts[v] = (counts[v] || 0) + 1;
  console.log(Object.keys(ENTRY_SHARDS).sort().join('\n'));
  process.stderr.write('per-shard counts: ' + JSON.stringify(counts) + '\n');
})" > /tmp/injection-plan.txt
diff /tmp/injection-ids.txt /tmp/injection-plan.txt && echo "plan covers every entry"
```

Expected: `diff` 无输出并打印 `plan covers every entry`；`per-shard counts` 中 1/2/3 三个值都 > 0。

- [ ] **Step 8: 提交**

```bash
git add .github/workflows/ci.yml scripts/verify/injection-suite.mjs scripts/verify/injection-shard-plan.mjs scripts/verify/injection-shard.mjs scripts/verify/checks/task-1.5.mjs
git commit -m "ci: 注入证明拆为 3 个分片并按实测成本显式分配"
```

---

### Task 6: 场景命令与任务脚本的 cargo 形态收敛

**Files:**
- Modify: `scripts/verify/run-scenario.mjs:60-65`
- Modify: `scripts/verify/checks/task-9.1.mjs:51-57,121-125`
- Modify: `scripts/verify/checks/task-16.1.mjs:39-45`
- Modify: `scripts/verify/checks/task-12.4.mjs:253-256`
- Modify: `.github/workflows/ci.yml`（injection 的 shell 构建）

**Interfaces:**
- Consumes: `groupedTestCommand(command)`（已存在）。
- Produces: 归一化后的执行命令 `cargo test --workspace --all-targets --all-features`，与 Linux 分片的 Test 步骤同形态，因此 `linux-workspace` 缓存可被 release-gate / perf / injection 复用。

- [ ] **Step 1: 在 `run-scenario.mjs` 中归一化**

在 `groupedTestCommand` 内、现有 vitest 分支之前加：

```js
  // Three registry commands select one package each with the same feature set.
  // The execution form is the workspace shape the Linux Test step builds, so a
  // restored `linux-workspace` cache serves it instead of recompiling per
  // package. Commands carrying extra arguments (a `--test` target or a filter)
  // are left alone: their shape is not the workspace shape.
  if (/^cargo test -p (?:echo-core|echo-desktop|echo-app) --all-features$/.test(command.trim())) {
    return "cargo test --workspace --all-targets --all-features";
  }
```

- [ ] **Step 2: 确认归一化生效且证据仍被观测到**

Run: `node scripts/verify/run-scenario.mjs -- --automated 2>&1 | tail -5`
Expected: 末行形如 `scenario batches: <N> commands for 561 scenarios (0 delegated)`；全部 `ok: scenario`，无 `evidence not observed`，退出 0。

归一化前有 **287** 条场景使用精确的 `cargo test -p (echo-core|echo-desktop|echo-app) --all-features` 形态。runner 末行的 `N` 是**去重后的执行命令数**（`executeScenarios` 的 `seenCommands` 按 execution command 去重），不是场景数：这 287 条场景本来就只对应 3 条包命令，归一化后 3 条并成 1 条，因此 `N` 从 **31 降到 29**（降 2）。覆盖映射减少 286 条是这个变化的场景级投影，不是末行 N 的差值。另 15 条带额外参数的 cargo 场景（`--test player_smoke`、`--release --test local_edit_performance`、`--ignored`、`file-drop-config.mjs &&` 复合命令等）不匹配该正则，命令保持原样——这是有意的：它们的形态本来就不是 workspace 形态。

- [ ] **Step 3: 收敛 `task-9.1.mjs`**

```js
run("cargo", ["check", "-p", "echo-app", "--locked", "--all-features"], "echo-app compiles");
const testOut = run(
  "cargo",
  ["test", "-p", "echo-desktop", "--lib", "--locked", "--all-features", "runtime::"],
  "echo-desktop runtime tests",
);
...
const appTestOut = run("cargo", ["test", "-p", "echo-app", "--locked", "--all-features", "open_targets"], "echo-app open_targets tests");
```

- [ ] **Step 4: 收敛 `task-16.1.mjs`**

```js
run("cargo", ["check", "-p", "echo-app", "--locked", "--all-features"], "echo-app compiles");
const testOut = run("cargo", ["test", "-p", "echo-app", "--locked", "--all-features", "open_targets"], "echo-app open_targets tests");
```

- [ ] **Step 5: 合并 `task-12.4.mjs` 的三次 vitest**

保持现有 `test -- --run <file>` 形态不变（该形态已在 CI 上工作），只把三次调用合并为一次——不要"顺手"去掉 `--`：

```js
run("pnpm", [
  "--filter", "@echo/desktop", "test", "--", "--run",
  "src/app/accessibility.test.tsx",
  "src/features/player/ImmersivePlayer.test.tsx",
  "src/features/player/PlayerBar.test.tsx",
], APP);
```

- [ ] **Step 6: 对齐 injection 的 shell 构建**

`ci.yml` 中 `run: cargo build -p echo-app` → `run: cargo build -p echo-app --all-features`。

同时修正 `task-gates` 上方注释的单复数（Task 5 已把 `the frontend shards run` 改为 `the frontend shard runs`，但谓语没跟着改）：
`# shards run typecheck/vitest/node checks and never invoke cargo.` 里的 `never invoke` → `never invokes`。

- [ ] **Step 7: 逐个验证**

```bash
node scripts/verify/checks/task-9.1.mjs
node scripts/verify/checks/task-16.1.mjs
node scripts/verify/checks/task-12.4.mjs
```

Expected: 三条都以 `ok 9.1:` / `ok 16.1:` / `ok 12.4:` 开头并退出 0。

- [ ] **Step 8: 提交**

```bash
git add scripts/verify/run-scenario.mjs scripts/verify/checks/task-9.1.mjs scripts/verify/checks/task-16.1.mjs scripts/verify/checks/task-12.4.mjs .github/workflows/ci.yml
git commit -m "ci: 场景命令与任务脚本收敛到 workspace 测试形态"
```

---

### Task 7: 重写 ci-structure 注入证明

**Files:**
- Modify: `scripts/verify/injection-governance.mjs:19-152`
- Modify: `scripts/verify/injection-shard-plan.mjs`（追加 7 个新 id + 注释行数）
- Modify: `scripts/verify/injection-shard.mjs`（删除 `?? entry.shard` 旁路）
- Test: `scripts/verify/injection-suite.mjs`

**Interfaces:**
- Consumes: Task 1–5 在 `task-1.5.mjs` 中定义的失败信息（本任务逐条匹配）。
- Produces: 16 条 `ci-*` 证明（现存 9 条：其中 6 条按本任务改写、3 条不动；再加 7 条新增），并把 7 个新 id 登记进 `injection-shard-plan.mjs`。

- [ ] **Step 1: 改写受影响的 6 条证明**

每条只改 `replaceIn` 的 needle/replacement 与 `expect`，id 与 `guard` 保留：

| id | needle / replacement | expect |
|---|---|---|
| `ci-platform-matrix/missing-linux-entry` | 删除 `ci.yml` 中 **rust 矩阵的** Linux 行（inline flow 形态，逐字为 `          - { platform: Linux,   os: ubuntu-latest,  shard: workspace, cache: linux-workspace,  packages: "--workspace",   clippy: true }\n`）。**不要**用旧的 `- platform: Linux\n  os: ubuntu-latest\n` —— 那会命中 `frontend` job 的块式矩阵（ci.yml:187），删除后触发的是 frontend 的 `missing platform 'Linux'`，与本条期望不符 | `/FAIL 1\.5: ci\.yml rust matrix is missing entry 'Linux\/workspace'/` |
| `ci-structure/test-condition-windows` | `"if: github.event_name != 'pull_request' || matrix.shard == 'workspace'"` → `"... || matrix.shard == 'core'"` | `/FAIL 1\.5: rust Test if must be 'github\.event_name != 'pull_request' \|\| matrix\.shard == 'workspace'/` |
| `ci-structure/rust-static-step-missing` | 删除整个 `Rust static gates` 步骤块（从 `      - name: Rust static gates` 到其后空行） | `/FAIL 1\.5: ci\.yml job 'rust' is missing step 'Rust static gates'/` |
| `ci-structure/test-condition-and-inverted` | `\|\|` → `&&`（同上 needle） | 同 `test-condition-windows` 的 expect |
| `ci-structure/test-continue-on-error` | needle `"      - name: Test\n        if: github.event_name != 'pull_request' \|\| matrix.shard == 'workspace'\n"` → 同一字符串 + `"        continue-on-error: true\n"` | `/FAIL 1\.5: rust Test must not set continue-on-error/` |
| `ci-structure/section-list-authority` | 不变（`ci-governance.mjs` 的 `"static", "coverage", "purity"` 仍在） | 不变 |

- [ ] **Step 2: 新增 7 条证明**

```js
{
  id: "ci-structure/shard-packages-drift",
  guard: "a rust shard's package selection drifting so the two shards no longer cover the workspace",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: ci\.yml rust matrix entry 'macOS\/core' maps packages to '-p echo-desktop', expected '-p echo-core'/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "shard: core,      cache: macos-core,       packages: \"-p echo-core\"",
      "shard: core,      cache: macos-core,       packages: \"-p echo-desktop\"",
    );
    return {};
  },
},
{
  id: "ci-structure/clippy-moved-after-test",
  guard: "Clippy running after Test, which would leave a check-shape build as the cached artifact",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: rust step order must be Format check < Task 16\.1 < Task 9\.7 9\.8 < Clippy < Test < Rust static gates/,
  baseline: true,
  inject() {
    const clippyBlock = "      - name: Clippy\n        if: matrix.clippy\n        run: cargo clippy --workspace --all-targets --all-features -- -D warnings\n\n";
    replaceIn(join(ROOT, ".github", "workflows", "ci.yml"), clippyBlock, "");
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "        run: cargo test ${{ matrix.packages }} --all-targets --all-features\n",
      `        run: cargo test \${{ matrix.packages }} --all-targets --all-features\n\n${clippyBlock}`,
    );
    return {};
  },
},
{
  id: "ci-structure/purity-section-removed",
  guard: "governance-static keeping the purity step name while dropping the purity section",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: governance-static purity step run must be 'pnpm verify:governance -- --sections=purity'/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "        run: pnpm verify:governance -- --sections=purity",
      "        run: pnpm verify:governance -- --sections=static",
    );
    return {};
  },
},
{
  id: "ci-structure/injection-shard-count-regressed",
  guard: "the injection proof matrix shrinking back to two shards",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: injection-proofs matrix\.shard must be \[1, 2, 3\]/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "        shard: [1, 2, 3]",
      "        shard: [1, 2]",
    );
    return {};
  },
},
```

再追加第 5、6 条：固定依赖审计的分层表达式，以及 desktop 分片在 main 上必须真实执行。

```js
{
  id: "ci-structure/audit-offline-tiering-frozen",
  guard: "the dependency audit's PR tiering pinned to a constant, which would silently disable the online scan on every event",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: governance-static audit step env ECHO_AUDIT_OFFLINE_ONLY must be/,
  baseline: true,
  inject() {
    // ```replaceIn``` is first-occurrence-only, and the same env line appears
    // twice in ci.yml — once under Install audit tooling, once under Verify
    // dependency audit gate. A bare env needle would rewrite the installer and
    // leave the step task-1.5 actually asserts on untouched, so the proof would
    // pass for the wrong reason. Carry the step name to make it unique.
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "      - name: Verify dependency audit gate\n        env:\n          ECHO_AUDIT_OFFLINE_ONLY: ${{ github.event_name == 'pull_request' && '1' || '0' }}\n",
      '      - name: Verify dependency audit gate\n        env:\n          ECHO_AUDIT_OFFLINE_ONLY: "1"\n',
    );
    return {};
  },
},
{
  id: "ci-structure/desktop-shard-gated-to-main",
  guard: "the macOS/Windows desktop shards gated so their workspace tests never run on main either",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: rust-desktop job if must be 'github\.event_name != 'pull_request'/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "  rust-desktop:\n    name: Rust desktop (${{ matrix.platform }})\n    runs-on: ${{ matrix.os }}\n    if: github.event_name != 'pull_request'\n",
      "  rust-desktop:\n    name: Rust desktop (${{ matrix.platform }})\n    runs-on: ${{ matrix.os }}\n    if: github.event_name == 'push'\n",
    );
    return {};
  },
},
{
  id: "ci-structure/job-if-references-matrix",
  guard: "a job-level if reaching for the matrix context, which GitHub evaluates before the matrix exists and rejects the whole workflow for",
  check: ["node", ["scripts/verify/checks/task-1.5.mjs"]],
  expect: /FAIL 1\.5: ci\.yml job 'rust-desktop' if must not reference the matrix context/,
  baseline: true,
  inject() {
    replaceIn(
      join(ROOT, ".github", "workflows", "ci.yml"),
      "    if: github.event_name != 'pull_request'\n",
      "    if: github.event_name != 'pull_request' || matrix.shard != 'desktop'\n",
    );
    return {};
  },
},
```

- [ ] **Step 3: 把 7 个新 id 登记进分片计划**

在 `scripts/verify/injection-shard-plan.mjs` 的 `ENTRY_SHARDS` 中追加（全部归 shard 1：它们只运行 `task-1.5`，是 node-only 的廉价证明）：

```js
  "ci-structure/shard-packages-drift": 1,
  "ci-structure/clippy-moved-after-test": 1,
  "ci-structure/purity-section-removed": 1,
  "ci-structure/injection-shard-count-regressed": 1,
  "ci-structure/audit-offline-tiering-frozen": 1,
  "ci-structure/desktop-shard-gated-to-main": 1,
  "ci-structure/job-if-references-matrix": 1,
```

不登记会让 `selectEntries` 抛 `entry '<id>' has no shard in injection-shard-plan.mjs`，三片全部失败。

- [ ] **Step 3b: 收紧分片计划的唯一权威**

1. `scripts/verify/injection-shard.mjs:17`：`const assigned = ENTRY_SHARDS[entry.id] ?? entry.shard;` → `const assigned = ENTRY_SHARDS[entry.id];`。删掉 `?? entry.shard` 这条遗留旁路——表外条目即使自带 `shard:` 也会被接受，等于保留两套权威、且改它不生效。
2. `scripts/verify/injection-shard-plan.mjs:3` 注释里的 `sits at 997 lines` → `sits at 996 lines`（Task 5 删掉了 `embedded-frontend/stale-binary` 的 `shard: 1` 一行）。
3. 追加 7 条后重新核对键集一致（防止陈旧键）：
```bash
node scripts/verify/injection-suite.mjs --list | sort > /tmp/injection-ids.txt
node -e "import('./scripts/verify/injection-shard-plan.mjs').then(({ENTRY_SHARDS}) => { console.log(Object.keys(ENTRY_SHARDS).sort().join('\n')) })" > /tmp/injection-plan.txt
diff /tmp/injection-ids.txt /tmp/injection-plan.txt && echo "plan covers every entry"
```
Expected: `plan covers every entry`，两文件各 52 行。

- [ ] **Step 4: 运行受影响的分片**

```bash
node scripts/verify/injection-suite.mjs --only ci-
```

Expected: 全部 `ok`，无 `FAIL`、无 `injection anchor not found`。

- [ ] **Step 5: 提交**

```bash
git add scripts/verify/injection-governance.mjs scripts/verify/injection-shard-plan.mjs scripts/verify/injection-shard.mjs
git commit -m "test(governance): 按新 CI 结构重写 ci-structure 注入证明"
```

---

### Task 8: 本地全量验证与耗时测量

**Files:** 无代码改动（验证任务）。

- [ ] **Step 1: 结构门禁**

```bash
node scripts/verify/checks/task-1.5.mjs
node scripts/verify/check-scale.mjs
node scripts/verify/check-scenario-evidence.mjs
```

Expected: 三条 `ok`；`check-scale` 不得报 `injection-suite.mjs` 超限。

- [ ] **Step 2: 三片注入证明各自跑通**

```bash
node scripts/verify/injection-suite.mjs --shard=1/3
node scripts/verify/injection-suite.mjs --shard=2/3
node scripts/verify/injection-suite.mjs --shard=3/3
```

Expected: 每片 `every registered gate failed exactly as claimed`，三片 `ok` 数之和为 52（45 + 7 新增）。记录每片耗时；若某片超过三片均值的 1.5 倍，按 Task 5 的规则搬条目后重跑。

- [ ] **Step 3: 治理门（独占运行）**

```bash
pnpm verify:governance
```

Expected: `ok governance: CI core gate passed`。运行时不得并行跑任何 cargo 或 rg 扫描（5 秒墙钟断言要求独占）。

- [ ] **Step 4: 分片测试各自独立跑一遍**

```bash
cargo test -p echo-core --all-targets --all-features
cargo test -p echo-sync -p echo-desktop -p echo-app --all-targets --all-features
```

Expected: 两条都通过（证明分片之间没有隐式顺序依赖）。

- [ ] **Step 5: 记录本地耗时基线**

```bash
node scripts/verify/run-scenario.mjs -- --automated 2>&1 | tail -3
/usr/bin/time -p node scripts/verify/checks/task-9.1.mjs
/usr/bin/time -p node scripts/verify/checks/task-12.4.mjs
```

Expected: 场景 561/561；task-9.1 与 task-12.4 的耗时写进 PR 描述作为对照。

> **归档顺序约束（R31）**：AGENT.md 要求「先同步并归档 change，再创建 PR」，因此 PR run 与 `main` 的墙钟观测不可能在归档前完成。本任务只负责本地基线；CI 观测放在 Task 9 的 Step 6/7，作为 PR 正文的核对清单在 run 产生后回填。`tasks.md` 8.3 的措辞已按此拆分。

- [ ] **Step 6: 提交（如有调整）**

```bash
git add -A
git commit -m "test(ci): 本地验证与分片耗时记录"
```

---

### Task 9: 文档同步、delta spec 同步与归档、PR

**Files:**
- Modify: `docs/arch-standards-remaining.md:390`
- Modify: `openspec/changes/ci-runtime-under-10-min/**`（勾选任务）
- Modify: `openspec/specs/engineering-governance/spec.md`（sync 后）

- [ ] **Step 1: 更新文档中的 CI 布局描述**

`docs/arch-standards-remaining.md` 的第 10 节是 2026-11-02 的历史记录，不要改写它。改为两步：

1. 在第 10 节标题 `## 10. CI 拓扑、分片与委托（2026-11-02，issue #124）` 之后插入一行引用块：`> 本节记录 2026-11-02 的拓扑。2026-11-17 起的拓扑见 §11。`
2. 在第 10 节末尾追加新的一节：

```markdown
## 11. CI 拓扑（2026-11-17，issue #<issue-number>）

| job（实例数） | 职责 | 关键点 |
|---|---|---|
| `Rust (<platform> <shard>)`（5） | fmt / clippy / 分片测试 | Linux `workspace`；macOS/Windows 各 `core` + `desktop`；PR 只跳过两个 `desktop` 分片 |
| `macOS native Gate`（1） | 8.12 / 13.2 | 自行 stage `target/Frameworks` 的 vendored dylib |
| `Task gates (core/desktop/frontend)`（3） | 42 个 task gate 分片 | 16.1 在 Rust Linux；1.7 在 governance-static |
| `Governance static gates`（1） | purity / registry / static / coverage / 依赖审计 | 依赖审计在线半仅在 main 与定时任务执行 |
| `Dependency audit`（独立 workflow，定时） | 1.7 在线半 | 每日 03:00 UTC + 手动触发 |
| `Injection proof 1/3`–`3/3`（3） | 注入证明分片 | 显式分片计划；shard 1 额外构建壳 |

缓存只有 6 个写者 key，且只在 main 写入；其余 job 只恢复不写入。
```

- [ ] **Step 2: 勾选 change 任务并运行 OpenSpec 校验**

```bash
npx openspec validate ci-runtime-under-10-min --strict
```

Expected: 校验通过。

- [ ] **Step 3: 同步 delta spec 并归档**

用 `openspec-sync-specs` 技能把 delta 同步到 `openspec/specs/engineering-governance/spec.md`，再用 `openspec-archive-change` 归档。归档后确认 `openspec/changes/ci-runtime-under-10-min` 已移入 `archive/`。

- [ ] **Step 4: 提交并推送**

```bash
git add docs/arch-standards-remaining.md openspec/
git commit -m "docs(openspec): 同步并归档 ci-runtime-under-10-min"
git push -u origin feat/ci-runtime-under-10-min
```

> **R2 覆盖**：不使用 `git add -A`（工作区有用户未提交的 `artifacts/` 改动）。只暂存本任务实际修改的路径。

- [ ] **Step 5: 创建 PR（中文，关联 Issue）**

```bash
gh pr create --title "ci: 压缩 CI 运行耗时至 10 分钟内并降低失败率" --body "$(cat <<'EOF'
Closes #<issue-number>

## 变更
- rust 矩阵拆为 5 个分片（Linux workspace / macOS core+desktop / Windows core+desktop），desktop 分片仅在 main 执行。
- 缓存按 6 个 shared-key 收敛，只有 main 写入；删除全部旧缓存。
- purity 与 dependency audit 并入 governance-static；在线公告扫描迁到 main + 每日定时 workflow。
- 注入证明 2 → 3 分片，task-gates 4 → 3 分片。
- 场景命令与任务脚本收敛到 `--workspace --all-targets --all-features` 形态。
- 结构门禁（task-1.5 / injection-governance）同步重写，新增 7 条注入证明。

## 验证
- [ ] `node scripts/verify/checks/task-1.5.mjs`
- [ ] `node scripts/verify/injection-suite.mjs --shard={1,2,3}/3`
- [ ] `pnpm verify:governance`（独占）
- [ ] 场景 561/561

## 目标
PR 与 main 的 CI 墙钟均 < 10 分钟；缓存总量 < 6 GB；main 上 5 个 rust 分片全部执行。
EOF
)"
```

- [ ] **Step 6: 观察 PR run 并记录**

```bash
gh run list --branch feat/ci-runtime-under-10-min --limit 1
gh run view <run-id> --json jobs --jq '.jobs[] | "\(.name)\t\(.conclusion)\t\((.completedAt|fromdate)-(.startedAt|fromdate))s"'
```

Expected: 18 个 job 实例；总墙钟 < 600s；`Rust (macOS desktop)` 与 `Rust (Windows desktop)` 为 skipped。

- [ ] **Step 7: 合并后确认 main 与缓存**

```bash
gh run list --branch main --limit 2
gh api repos/zsxink/Echo/actions/cache/usage
```

Expected: main run 20 个实例、5 个 rust 分片全部 executed、总墙钟 < 600s；第二次 run 的 Rust job 日志出现主键 `Cache restored`（不是 restore-key）；缓存总量 < 6 GB。

> **合并须用户确认**：`gh pr merge` 是对外动作，本计划只授权创建 PR 与推送分支；合并前必须取得用户明确同意。Step 7 的 main 观测在合并后执行。

## 回退顺序（来自设计 §11）

1. Linux 关键路径 > 560s：把 `Rust static gates` 移到 `release-gate` 的场景步骤之后。
2. main 仍 > 10 分钟且瓶颈是 Windows：把 `desktop` 分片再拆成 `desktop` 与 `app`（实例 +1，从 `task-gates frontend` 借一个）。
3. 缓存仍不命中：把 6 个 shared-key 收敛为 3 个（每平台一个），接受 reader job 的部分重建。
