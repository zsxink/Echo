# Design

## Context

动机与基线见 `proposal.md — Why`。完整测量与推导记录在
`docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md`，本文件给出实施所需的决策与契约。

现状约束（全部在当前 HEAD 实测）：

- PR run `41323932057` 墙钟 21m46s；Task gates 1304s、Governance 1015s、Rust Windows 983s、
  Rust macOS 830s、Release gate 827s、Injection 598s、Rust Linux 420s。
- CI 形状被脚本反向锁定：`task-1.5` 断言 `ci.yml` 的精确命令；`task-crp` 锁定 release 结构；
  `injection-governance` 用注入证明守护治理门；`check-scale` 限制单文件 1000 行。
- `validate-scenario-commands` 与 `check-scenario-evidence` 的
  `cargo test -p <pkg> --all-features -- --list` 没有 `--all-targets`，会构建 doc-test harness；
  基线中这一项 323s。本机加 `--all-targets` 后热构建 0.9s。
- `main` 分支保护当前 `required_status_checks: null`，`enforce_admins: true`。job 重命名不会
  阻止合并，也不会因 CI 红自动阻止合并；验收必须以 `gh run view <id>` 的 job/step 记录为准。

本 change 只改验证层（workflow 与 `scripts/verify`），不修改任何生产代码；不新增跨层依赖，
Core / 平台层的依赖方向不变。脚本改动遵守 `openspec/CODE_STANDARDS.md` 与 `check-scale` 的
1000 行上限。

## Goals / Non-Goals

**Goals:**

- PR 墙钟 < 10 分钟（主口径：rust-cache 命中的常态 run），18 个 job，关键路径 480–520s。
- 523 场景 / 541 证据 / 注入证明数量 / 43 个 task 一项不减。
- 分层后的覆盖缺口可被结构门禁发现，而不是依赖人工记忆。
- 本地 `pnpm verify:governance` 的默认行为不变（仍跑全集）。

**Non-Goals:**

- 不收敛前端三平台矩阵（70–80s，不是瓶颈）。
- 不合并 dependency-audit（38s，留作并发不足时的回退杠杆）。
- 不引入 nextest、不改变 cargo 编译参数、不放宽 `-D warnings` / coverage / scale / purity。
- 不重构 `release.yml` 或发布流程。

## Decisions

### D1 — PR 与 main 分层覆盖

三平台 `rust` job 都保留 `cargo fmt` 与 `cargo clippy --workspace --all-targets --all-features
-- -D warnings`。完整测试步骤改为：

```yaml
if: github.event_name != 'pull_request' || matrix.platform == 'Linux'
run: cargo test --workspace --all-targets --all-features
```

PR 只有 Linux 跑完整工作区测试；push 到 `main`、merge queue、手动触发等非 PR 事件三平台都跑。
命令从 `--all-features` 改为 `--all-targets --all-features`，与原先 Governance 的
`quality-core tests` 覆盖并集一致；仓库没有 doctest，`--all-targets` 不丢 doc 测试。
macOS 的 8.12/13.2 与三平台 platform Gate 仍在 PR 上执行。

- 备选：PR 上保留 Windows/macOS 全量测试，只优化其它 job。否——983s / 830s 仍在关键路径附近，
  无法稳定压到 10 分钟；用户已确认接受该分层。
- 备选：PR 上 Windows/macOS 只跑受影响 crate 的测试。否——需要引入路径过滤与依赖图分析，
  误判会静默漏测；分层到 main 更简单且可被结构门禁验证。

### D2 — Task gates 拆成 4 个显式分片

用 `strategy.matrix.include` 定义四个 shard，每个 shard 显式列出 task id，不引入新的 runner 逻辑。
分片按依赖与语言分组，避免每个 shard 都装 webkit / 构建前端。

| Shard | Tasks | 需要 Rust | 说明 |
|---|---|---|---|
| `core` | `1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3` | 是 | 先跑 cargo 重的 6.1/13.3/11.5/12.5；12.5 的 benchmark 产物在此上传 |
| `desktop` | `7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2` | 是 | 7.1 先建立 `echo-desktop` 构建，11.4 复用 |
| `frontend-a` | `10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1` | 否 | 13.1 需要 Chrome；设置 `CHROME_PATH` / `CHROME_NO_SANDBOX` |
| `frontend-b` | `10.7 10.8 10.9 10.10 12.1 12.2 12.3` | 否 | 纯前端；不安装 Rust |

- `16.1` 从 task-gates 移到 Rust Linux：它在完整工作区测试之后运行，`echo-app` 已构建，
  `cargo check` / `open_targets` 测试只需链接或复用。
- task-gates 不再安装 webkit、不再构建前端。当前注释声称 16.1/16.2 会编译 `echo-app`，
  实际只有 16.1 会；16.1 移走后该依赖消失。若后续新增 task 需要 shell，必须显式在该 shard
  加 `webkit` / `frontend` 标志，而不是恢复全局构建。
- 4 个 shard 的并集是 42 项；加上 Rust Linux 的 `16.1` 恰好覆盖 43 项。`task-1.5` 维护
  固定 43 项常量并断言这一分解。

- 备选：按 task 编号均匀切成 4 份。否——会把需要 webkit 与纯前端 task 混在一起，每个 shard
  都要装全套依赖，setup 成本抵消分片收益。
- 备选：每个 task 一个 job。否——43 个 job 超出 runner 并发，排队会拉长墙钟。

### D3 — Governance 按环境拆段

`ci-governance.mjs` 的检查表增加 `section` 字段，CLI 支持 `--sections=a,b` 与 `--list-sections`；
未知 section 直接失败。默认（不传）运行全部 section，本地体验不变。

| Section | 检查 | CI 执行位置 |
|---|---|---|
| `quality` | fmt、clippy、workspace tests | 不通过 governance 跑；由 `rust` 矩阵覆盖（PR 三平台 fmt/clippy，Linux/non-PR 测试） |
| `registry` | reconcile、gen-scenario-manifests | `governance-static` |
| `rust-static` | validate-scenario-commands、scenario evidence | `rust-linux`，在完整测试之后 |
| `static` | scale、fixture determinism、lint inheritance、toolchain pin、verification validity、scenario command proof | `governance-static` |
| `coverage` | task-12.8 `cargo llvm-cov` | `governance-static` |
| `purity` | build purity | `rust-linux`，`ECHO_PURITY_REQUIRE_BUILD=1` |
| `embedded` | embedded frontend freshness | `injection-shard-1`（shell 构建之后） |
| `injection` | injection suite | `injection-shard-1/2`（本地默认仍整跑） |

`validate-scenario-commands` 与 `check-scenario-evidence` 依赖 cargo 测试列表；把它们放到 Rust Linux
的完整测试之后，构建已热。`coverage` 留在 governance-static，因为它自己用 `cargo llvm-cov` 构建，
不需要 webkit 或前端。`governance-static` 不再安装 webkit、不再构建前端；保留 ffmpeg（fixture
检查）与 coverage 工具。

- 备选：把 governance 整门留在单 job，只并行 task gates。否——Governance 1015s 仍是第二关键路径，
  且与 Rust Linux 重复 fmt/clippy/test。
- 备选：把 `rust-static` 留在 governance-static。否——会冷构建测试二进制；放到 Rust Linux 是
  本设计的主要收益来源之一。

### D4 — Release gate 只跑场景，性能场景独立

- `task-13.9.mjs` 新增 `--skip-static`：跳过 reconcile / validate / gen；`--delegated=` 继续
  透传给 `run-scenario.mjs`。
- Release gate 命令改为：

  ```text
  node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf
  ```

- 三个性能场景（`SFI-R05-S03`、`LE-R21-S02`、`PM-R13-S05`）在 `scenario-commands.mjs` 标记
  `delegatedTo: "perf"`；新增 `package.json` 脚本：

  ```json
  "verify:perf": "node scripts/verify/run-scenario.mjs SFI-R05-S03 LE-R21-S02 PM-R13-S05"
  ```

- 新增 `perf` job：Rust + rust-cache + setup-node + pnpm（不装 webkit、不装 node 依赖、不构建前端），
  运行 `pnpm verify:perf`。三条场景共享同一条 `PERF_ALL` 命令，`run-scenario` 去重为一次执行。
- `check-scenario-evidence.mjs` 的 `DELEGATED_GATES` 加入 `perf`；它用 `pnpm verify:perf` 在
  `ci.yml` 中存在来证明该 gate 真的接了电。

**成本修正**：`--skip-static` 去掉的是重复的静态校验与 doc-test 构建，不是场景本身需要的编译。
Release gate 的第一条场景命令仍要构建一次 per-package 测试二进制（约 150–200s），随后场景复用。
预期 **480–520s**，不是 340s；`--skip-static` 的真实收益约 250s。修正后的估算仍低于 10 分钟，
但它是 PR 关键路径之一，必须按此验收。

- 备选：Release gate 继续跑性能场景。否——性能场景 220s 会把 release gate 推过 10 分钟；
  独立 job 可并行且不改变场景断言。
- 备选：把 release gate 拆成 2 个场景 shard。否——场景之间共享测试二进制，拆开会各自冷编译；
  实测超过 560s 时再作为回退项。

### D5 — 注入证明拆成 2 个 shard

- 新增 `scripts/verify/injection-shard.mjs`，导出 `selectEntries(entries, { only, shard })`：
  按 `entry.shard ?? (index % total) + 1` 分配；`--shard=<i>/<n>` 选择第 i 片；非法 spec、越界、
  空片都失败。`selectEntries` 每次调用同时计算 1..n 各片的并集，断言并集等于全集且各片互斥；
  分片公式写错会让每个 shard 都红，而不是静默漏跑。
- `injection-suite.mjs` 只增加解析 `--shard=` 与调用 helper 的几行，保持文件在 1000 行以内
  （当前 987 行）。若仍超限，先把现有 `ENTRIES` 拆到新文件，作为独立机械重构。
- 嵌入前端新鲜度的注入条目显式 `shard: 1`，因为只有 shard 1 构建 shell 二进制；其余条目按稳定
  顺序轮转。
- 两个 shard 都做全量完整性检查（`ENTRIES` 全集），因此不会因为分片而漏掉「某检查没有证明」。
- shard 1 构建 shell 后额外运行一次正向的 `check-embedded-frontend.mjs`（`pnpm verify:frontend-fresh`）；
  shard 2 不构建 shell，只构建前端（部分注入条目会 `cargo check -p echo-app`，需要 `dist`）。
- 两个 shard 都安装 Rust 工具链、rust-cache、webkit 与 pnpm 依赖：shard 2 的注入条目会
  `cargo check -p echo-app`，Linux 上同样需要 webview 头文件。

- 备选：按注入类别手工分配（cargo / node / frontend）。否——类别边界会随条目演化，手工清单
  容易漏；轮转 + 显式 `shard` 覆盖特例更稳。
- 备选：只跑 shard 1，跳过 shard 2。否——会减少注入证明覆盖，违反不变式。

### D6 — 脚本级去重

1. **`self-test.mjs` 修复真实 cargo 泄漏**：最后一条 lockfile 不变性运行
   `runRunner(fixture, ["ok", "failing", "--all"])` 时传入 `taskEnv`（fake cargo / fake pnpm 的 PATH），
   使 fixture 里的 `multi-name` / `missing-name` 不再调用真实 cargo。1.3 从 171s 降到数秒。
   判定标准不变：self-test 仍须覆盖 unknown id、missing command、failing command、missing evidence、
   零测试、未观测证据、lockfile 不变。
2. **统一 feature / target 集**：task 检查中只针对 lib 模块的 cargo 调用加 `--all-features --lib`
   （`8.1`、`8.2`、`10.6`、`11.1`、`11.2`、`11.4`、`11.5`、`12.5`、`13.3`，以及 `task-6.mjs` /
   `task-7.mjs` 的循环）；`1.8` 保留 `--test logging`；`16.2` 已有 `--all-features --lib --locked`。
3. **LFS 只留在 Windows**：仓库唯一的 LFS 对象是
   `vendor/libmpv/windows/libmpv-2.dll`。Linux/macOS 的 checkout 去掉 `lfs: true`；Windows rust job
   与 Windows platform Gate 保留。macOS 的 vendored dylib 不是 LFS。
4. **`--skip-static` / `--sections` 只改变执行位置**：不改变任何检查的断言；本地默认仍执行全集。
5. **对齐 `cargo test --list` 的构建形态**：`validate-scenario-commands.mjs` 与
   `check-scenario-evidence.mjs` 的调用改为
   `cargo test -p <pkg> --all-targets --all-features -- --list`。若某个 package 的 bench/example
   在 `--list` 下不可用，退回 `--lib --tests --all-features`（同样跳过 doc-tests），并保持与
   测试步骤同构。
6. **section 全覆盖守卫**：`ci-governance.mjs` 增加 `--list-sections`，`task-1.5` 把 `ci.yml` 中
   所有 `--sections=` 选择、injection/embedded 的直接调用，以及 rust 矩阵的 fmt/clippy/Test 步骤
   合并，断言每个 section 至少被执行一次。映射固定为：`quality` → rust 矩阵三步；
   `embedded` → `pnpm verify:frontend-fresh`；`injection` → `injection-suite.mjs --shard`；
   其余 → 对应 job 的 `--sections=` 列表。新增 section 而没有接线时，`task-1.5` 直接失败。

- 备选：用 `cargo metadata` / JSON helper 生成测试列表。否——`--all-targets -- --list` 已足够且
  与测试步骤同构，引入新 helper 会增加维护面。
- 备选：保留 LFS 全平台 checkout。否——非 Windows 平台下载无关的大文件，setup 变慢且无收益。

### D7 — 前端矩阵保持不变

`frontend` 三平台 job 继续跑 format/lint/typecheck/test/build。它只占 70–80s，不是瓶颈；
削减它会违反工程治理规格的稳定平台语义。

### D8 — Linux platform Gate 并入 Rust (Linux)

`linux-platform-gate` 当前只跑 `pnpm verify:task -- 9.7 9.8`，基线 26s。把这两项追加到 Rust (Linux)
的测试后步骤，删除独立 job；断言内容不变。Rust (Linux) 的步骤顺序固定为：

```text
fmt → clippy → Test → Rust static gates → Task 16.1 → Task 9.7 9.8
```

Windows / macOS platform Gate 不变。

### 目标 CI 结构

PR 上共 18 个 job（矩阵展开后），18 < GitHub Free 公共仓库的 20 个并发上限。

| # | Job | 事件 | Runner | 关键步骤 | 预计墙钟 |
|---|---|---|---|---|---:|
| 1 | Rust (Linux) | PR + main | ubuntu | fmt、clippy、全量 test、`--sections=rust-static,purity`、16.1、9.7/9.8 | 480–520s |
| 2 | Rust (macOS) | PR + main | xcode-27 | fmt、clippy；PR 上 8.12/13.2；non-PR 加全量 test | 325s（PR）/ 810s（main） |
| 3 | Rust (Windows) | PR + main | windows | fmt、clippy；non-PR 加全量 test | 120s（PR）/ 955s（main） |
| 4–6 | Frontend (Linux/macOS/Windows) | PR + main | 三平台 | format/lint/typecheck/test/build | 70–80s |
| 7 | Task gates (core) | PR + main | ubuntu | 10 个 task + benchmark artifact | 300s |
| 8 | Task gates (desktop) | PR + main | ubuntu | 17 个 task | 350s |
| 9 | Task gates (frontend-a) | PR + main | ubuntu | 8 个 task，含 13.1 Chrome | 275s |
| 10 | Task gates (frontend-b) | PR + main | ubuntu | 7 个 task | 255s |
| 11 | Dependency audit | PR + main | ubuntu | 1.7 | 38s |
| 12 | macOS platform Gate | PR + main | xcode-27 | 1.9/1.10/9.7 | 306s |
| 13 | Windows platform Gate | PR + main | windows | 原生确认 + 9.7/9.8/17.1/18.1 | 313s |
| 14 | Governance static | PR + main | ubuntu | `--sections=registry,static,coverage` | ~390s |
| 15 | Injection proof 1/2 | PR + main | ubuntu | 前端构建、shell 构建、`--shard=1/2`、正向 embedded 检查 | ~390s |
| 16 | Injection proof 2/2 | PR + main | ubuntu | 前端构建、`--shard=2/2` | ~310s |
| 17 | Release gate | PR + main | ubuntu | `task-13.9 --skip-static --delegated=governance,perf` | 480–520s |
| 18 | Performance scenarios | PR + main | ubuntu | `pnpm verify:perf` | ~260s |

PR 关键路径是 Rust (Linux) 与 Release gate 的 480–520s，其次是 Governance static / Injection 1 /
Task gates (desktop) 的 350–390s。加 10–30s 的 job 启动与排队，PR 预计 **8.5–9.8 分钟**；
10 分钟是验收上限。main 由 Rust (Windows) 的 ~955s 决定，预计 16 分钟左右（附带收益，不作为
验收指标）。

### 脚本契约

| 文件 | 变更 |
|---|---|
| `ci-governance.mjs` | 检查表增加 `section`；新增 `--sections=a,b`、`--list-sections`；未知 section 失败；保留 `--without-injection` 兼容 |
| `run-scenario.mjs` | `--delegated` 支持逗号分隔多 gate；只跳过显式声明对应 gate 的场景 |
| `task-13.9.mjs` | 新增 `--skip-static`；最终信息不得声称已做静态全集校验 |
| `validate-scenario-commands.mjs` | `cargo test --list` 加 `--all-targets` |
| `check-scenario-evidence.mjs` | 同上；`DELEGATED_GATES` 加入 `perf` |
| `scenario-commands.mjs` + `manifest.json` | 三条性能场景 `delegatedTo: "perf"` |
| `package.json` | 新增 `verify:perf` |
| `self-test.mjs` | fixture 使用 fake PATH；覆盖多 gate 委托 |
| `task-1.5.mjs` | 增加步骤级 `run` / `if` / `env` 断言；43 项 task 分解断言；section 全覆盖断言 |
| `injection-shard.mjs` | 新增；分片选择与并集 / 互斥校验 |
| `injection-suite.mjs` | 解析 `--shard=`，调用 helper；嵌入条目 `shard: 1` |
| `injection-governance.mjs` | 新增 3 条证明：Test 条件误放到 Windows、删除 `Rust static gates`、从 `--sections` 移除 `coverage` |

### 覆盖映射与防退化

| 原检查 | 新位置 | 不静默保证 |
|---|---|---|
| Governance quality（fmt/clippy/test） | 三平台 rust fmt/clippy + Linux/non-PR 全量 test | `task-1.5` 断言命令与事件条件 |
| reconcile / gen | governance-static `registry` | `task-1.5` 断言 `--sections=registry,static,coverage` |
| validate / evidence | rust-linux `rust-static` | `task-1.5` 断言步骤存在与 `if`；`check-scenario-evidence` 证明委托 gate 接了电 |
| scale / fixture / lint / toolchain / validity / proof | governance-static `static` | section 全覆盖断言 |
| coverage | governance-static `coverage` | section 全覆盖断言；注入证明删除 `coverage` 会红 |
| purity | rust-linux `purity`，`ECHO_PURITY_REQUIRE_BUILD=1` | `task-1.5` 断言 env |
| embedded | injection-shard-1 正向检查 | 注入条目 `shard: 1` + 正向执行 |
| injection | 两片 `--shard` | 每片都做全集完整性检查；分片并集 / 互斥校验 |
| Linux platform Gate | Rust (Linux) 的 9.7/9.8 | `task-1.5` 断言步骤存在；原 task 断言不变 |
| 16.1 | Rust (Linux) | `task-1.5` 断言 `run` 与 `if`；43 项分解断言 |

## Migration Plan

1. 从 `main` 拉分支 `codex/issue-124-ci-pr-under-10-min`。
2. 按 tasks.md 的顺序逐组提交；每组独立可回滚。
3. 先做脚本契约与静态门（不改变 CI 行为），再改 `ci.yml` 结构；最后跑本地治理门与 PR 实测。
4. 若 PR 实测超过 10 分钟，按以下顺序回退：
   - 把 `rust-static` 移到 Release gate 的场景步骤之后（此时已热）；
   - 减少 task shard `core` 的串行项（把 12.5 移到独立 job）；
   - 合并 dependency-audit 到 governance-static；
   - 若 Release gate > 560s，拆成 2 个场景 shard。
5. 全部完成 → 同步 delta spec → 归档 change → 创建 PR。

## Risks / Trade-offs

- **双关键路径**（Rust Linux / Release gate 各 480–520s）→ 冷缓存或 `--all-targets` 变慢会逼近
  10 分钟；按 Migration Plan 的回退顺序处理，并在 CI 记录第一步耗时。
- **`validate-scenario-commands` / `check-scenario-evidence` 在 Rust Linux 上仍慢** → 已定位为
  doc-test 构建；若 CI 仍 > 60s，回退到 `--lib --tests`。
- **Release gate 首条场景命令承担测试二进制构建** → 估算已修正为 480–520s；验收单独记录该步。
- **冷缓存不作为失败条件** → 主口径是 rust-cache 命中的第二次 run；冷缓存 run 单独记录。
- **18 个 job 接近并发上限** → 18 < 20；若出现排队，先合并 dependency-audit。
- **`task-1.5` 与 CI 文本强耦合** → 断言按 job/step 名定位并给出精确失败原因；这是仓库既有的
  「CI 形状反向断言」纪律。
- **新增 Linux 专属步骤漏写 `matrix.platform == 'Linux'`** → `task-1.5` 逐步骤断言；注入证明覆盖
  `Test` 条件。
- **task 分片遗漏 / 重复** → `task-1.5` 断言 42 项并集 + 16.1，无重复。
- **`--all-targets` 增加 Linux 测试编译时间** → 与 governance 原覆盖一致；仓库无 doctest；实测
  超预算时只保留 `--all-features`，把 bench 编译留给 task-12.5。
- **`self-test` 用 fake cargo 后可能掩盖真实 lockfile 行为** → self-test 仍断言 lockfile SHA-256；
  真实 `pnpm install --frozen-lockfile` 仍在 1.3 中执行；fake cargo 只替代 fixture 的 task-4 调用。
- **`injection-suite` 接近 1000 行** → shard 逻辑放新 helper；suite 只加 import 与调用。
- **注入 shard 2 缺少 shell 构建** → shard 2 仍构建前端；只有 `cargo build -p echo-app` 被跳过；
  嵌入前端条目固定 `shard: 1`。
- **Windows/macOS 回归在 PR 上漏过** → 用户已接受；平台 Gate、clippy 与 main 完整测试兜底；
  PR 模板/文档注明该分层。

## Open Questions

（无。冷缓存验收口径、Release gate 估算、两个 injection shard 的依赖安装、section 覆盖守卫
均已在设计阶段裁定，见 `docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md — §11`。）
