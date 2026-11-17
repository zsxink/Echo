# CI 运行耗时压缩设计（PR 与 main 双 10 分钟）

- 日期：2026-11-16
- 状态：待用户批准（未创建 Issue、未进入 OpenSpec propose、未改动任何 workflow）
- 目标 change 名（暂定）：`ci-runtime-under-10-min`
- 前序设计：`docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md`（该设计只承诺 PR，且把 main 明确列为非目标）
- 相关规格：`openspec/specs/engineering-governance/spec.md`（CI 执行分层与结构完整性、CI status checks use stable platform names）

## 1. 背景与目标

2026-11-02 的设计把 PR 目标定为 10 分钟内，并记录了一次 14m52s 的实测。此后耗时回升：最近 30 天 PR 平均墙钟约 17.5 分钟，最近一次 main run（`43827801014`，2026-11-16）墙钟 **23 分 47 秒**。

本次目标（用户已确认）：

1. **PR 与 main 的 CI 墙钟都稳定低于 10 分钟**；
2. **降低失败率**；
3. 不删除任何检查、不降低断言强度、不缩小平台覆盖。

`2026-11-02` 设计把 main 排除在目标外，本次把它纳入，因此需要比上次更彻底的并行化。

## 2. 基线测量

### 2.1 最近一次 main run（`43827801014`，18 个 job 实例）

| Job | 墙钟 | 关键步骤 |
|---|---:|---|
| Rust (macOS) | 1423s | Test 809s、Clippy 217s、8.12/13.2 285s |
| Rust (Windows) | 1411s | **Test 1018s**、Clippy 188s |
| Rust (Linux) | 1194s | Test 476s、Rust static gates 469s、Clippy 129s |
| Release gate | 853s | 门禁步骤 787s |
| Injection proof 1/2 | 822s | shell 构建 203s、注入证明 506s |
| Injection proof 2/2 | 629s | 注入证明 469s |
| macOS platform Gate | 397s | 1.9/1.10/9.7 |
| Windows platform Gate | 364s | 原生确认 + 9.7/9.8/17.1/18.1 |
| Task gates (desktop) | 360s | — |
| Governance static | 338s | registry,static,coverage |
| Performance | 320s | — |
| Task gates (core) | 302s | — |
| Task gates (frontend-b) | 296s | — |
| Dependency audit | 289s | `cargo install` 占 247s |
| Task gates (frontend-a) | 146s | — |
| Frontend (Windows/macOS/Linux) | 133/84/54s | — |

### 2.2 Test 步骤内部：编译占了 60–80%

从 run 日志按时间戳切分（首次 `test result:` 之前为编译）：

| 平台 | Test 步骤 | 编译 | 执行 | 编译占比 |
|---|---:|---:|---:|---:|
| Windows | 1018s | 710s | 358s | 70% |
| macOS | 809s | 601s | 235s | 74% |
| Linux | 476s | 390s | 100s | 82% |

本地实测（macOS，热构建 + `CARGO_INCREMENTAL=0`，干净 `CARGO_TARGET_DIR`）复现了同一现象：完整 workspace 构建完成后，再跑一次 `cargo test -p <pkg> --all-targets --all-features -- --list` 仍要 **143s** 重建。

### 2.3 Release gate 内部（29 条去重命令，787s）

| 命令 | 耗时 |
|---|---:|
| `cargo test -p echo-core --all-features` | 259s |
| `node task-9.1.mjs`（内部 `cargo check -p echo-app --locked` 等） | 216s |
| `node task-13.8.mjs`（内部 `cargo test -p echo-desktop --all-features <filter>`） | 102s |
| `node task-12.4.mjs`（三次独立 vitest） | 72s |
| `pnpm --filter @echo/desktop test` | 46s |
| 其余 24 条 | 合计约 155s |

### 2.4 缓存实况（本次根因的直接证据）

`gh api repos/zsxink/Echo/actions/cache/usage`：

```text
active_caches_size_in_bytes = 10,878,794,132   (10.13 GB)
active_caches_count         = 14
```

GitHub Free 每仓库缓存上限 **10 GB**，已超限，14 条 Rust 缓存互相淘汰。

同一次 run 的缓存日志：

| Job | 缓存日志 |
|---|---|
| Rust (Linux) | `No cache found.` |
| Rust (macOS) | `No cache found.` |
| Rust (Windows) | `Cache hit for restore-key: v0-rust-rust-Windows_NT-x64-...`（回退键，非主键） |

也就是说：**三平台 Rust job 在同一次 run 里都没有命中自己的缓存**。Test 步骤因此在 Clippy 刚编译过同一张依赖图之后，仍重新编译了 318（Windows）/315（macOS）/413（Linux）个 crate——包括 `cfg-if`、`itoa` 这类永不变化的叶子依赖。

## 3. 根因

### R1：缓存预算溢出（首要根因）

18 个 job 各自保存一份 0.2–1.3 GB 的 Rust target 缓存，总量 10.1 GB > 10 GB 上限；PR run 的缓存按 PR ref 作用域保存，无法被 main 复用，却同样占用配额。结果是最需要缓存的 `rust` 三平台 job 反而拿不到缓存，每次 run 支付 390–710s 的冷编译。

### R2：同一个 job 内 clippy（check 形态）与 test（build 形态）互相作废

`cargo clippy --workspace --all-targets --all-features` 只产出 rmeta，不产出 rlib/object；紧随其后的 `cargo test` 必须为整张图重新 codegen。这个顺序在每个平台上都发生（Windows 201 + 318 次编译）。

### R3：单个 job 串行堆叠了多条关键路径

`Rust (Linux)` 一个 job 里串了 fmt、clippy、test、静态门禁、16.1、9.7/9.8；`Rust (macOS)` 串了 clippy、test、8.12/13.2。墙钟是相加，不是取最大。

### R4：脚本内部的异形 cargo 调用

`task-9.1`（216s）用 `cargo check -p echo-app --locked`、`cargo test -p echo-desktop --lib --locked`；`task-16.1` 用 `cargo check -p echo-app --locked`。默认 feature、`--lib`、`--locked` 的组合与 `--workspace --all-targets --all-features` 不同形态，各自触发重建。Release gate 的场景命令是 `cargo test -p <pkg> --all-features`（无 `--all-targets`），也与工作区形态不同。

### R5：失败率来源

159 次 run 中 62 次至少有一个 job 失败（已排除被 concurrency 取消的 job）。真实失败计数前列：

| Job | 失败次数 | 主要形态 |
|---|---:|---|
| Release gate | 19 | `evidence not observed`、PRD matrix |
| Governance（旧名，现拆为 governance-static + rust-static） | 18 | 静态门禁 |
| Injection proofs | 11 | 注入证明与 CI 结构耦合 |
| Rust (Windows) | 9 | 超时与墙钟敏感测试 |
| Dependency audit | 7 | 新 RUSTSEC 公告（真实信号，但阻塞所有 PR） |

前四类都与「冷缓存 → 单 job 负载过高 → 墙钟断言/超时」同源。R1/R3 修好后它们应同步下降。

## 4. 设计原则

1. **一个缓存 key = 一个固定 cargo 形态。** 会写缓存的 job，其最后一条 cargo 调用必须与缓存内容同形态；其余 job 只读不写。
2. **只有 main 能写缓存。** PR run 的缓存无法跨 run 复用，写它纯粹消耗 10 GB 配额。
3. **墙钟取最大，不取和。** 同一平台的所有检查并行到独立 job；单个 job 只做一件事。
4. **job 实例数不超过 20。** GitHub Free 的并发上限是 20；超过就会排队，把并行收益还回去。macOS 实例不超过 5。
5. **不删检查、不放宽断言。** 只做迁移、分片、去重、缓存。

## 5. 目标结构

### 5.1 Job 清单（20 个实例；PR 上 18 个）

| # | Job | 实例 | 事件 | 预计墙钟 | 缓存角色 |
|---|---|---:|---|---:|---|
| 1 | Rust (Linux workspace) | 1 | PR + main | ~440s | 写 `linux-workspace` |
| 2 | Rust (Windows core) | 1 | PR + main | ~430s | 写 `windows-core` |
| 3 | Rust (Windows desktop) | 1 | main only | ~360s | 写 `windows-desktop` |
| 4 | Rust (macOS core) | 1 | PR + main | ~390s | 写 `macos-core` |
| 5 | Rust (macOS desktop) | 1 | main only | ~305s | 写 `macos-desktop` |
| 6 | Frontend (Linux/macOS/Windows) | 3 | PR + main | 54/84/133s | 无 |
| 7 | Task gates (core/desktop/frontend) | 3 | PR + main | 302/360/442s | 读 `linux-workspace` |
| 8 | Governance static | 1 | PR + main | ~380s | 写 `governance-static` |
| 9 | macOS platform Gate | 1 | PR + main | 397s | 读 `macos-desktop` |
| 10 | macOS native Gate | 1 | PR + main | ~385s | 读 `macos-desktop` |
| 11 | Windows platform Gate | 1 | PR + main | 364s | 读 `windows-desktop` |
| 12 | Performance scenarios | 1 | PR + main | 320s | 读 `linux-workspace` |
| 13 | Release gate | 1 | PR + main | ~310s | 读 `linux-workspace` |
| 14–16 | Injection proof 1/3、2/3、3/3 | 3 | PR + main | ~340/325/325s | 读 `linux-workspace` |

关键路径预计 ~442s（Task gates frontend），加 20–30s 排队，**PR ≈ 7.5–8 分钟，main ≈ 8 分钟**。

### 5.2 `rust` 矩阵定义

```yaml
rust:
  name: Rust (${{ matrix.platform }} ${{ matrix.shard }})
  strategy:
    fail-fast: false
    matrix:
      include:
        - { platform: Linux,   os: ubuntu-latest,  shard: workspace, cache: linux-workspace,  packages: "",                                            clippy: true }
        - { platform: macOS,   os: xcode-27,       shard: core,      cache: macos-core,       packages: "-p echo-core",                                clippy: true }
        - { platform: macOS,   os: xcode-27,       shard: desktop,   cache: macos-desktop,    packages: "-p echo-sync -p echo-desktop -p echo-app",    clippy: false }
        - { platform: Windows, os: windows-latest, shard: core,      cache: windows-core,     packages: "-p echo-core",                                clippy: true }
        - { platform: Windows, os: windows-latest, shard: desktop,   cache: windows-desktop,  packages: "-p echo-sync -p echo-desktop -p echo-app",    clippy: false }
```

- `desktop` 分片在 `pull_request` 上整体跳过（规格允许）。
- `core` 分片在 PR 上只跑 fmt + clippy，在 main 上再跑测试。
- 三个平台合起来仍然覆盖**全部 workspace 包**，`cargo test --workspace` 的覆盖没有减少。
- `core` 分片不依赖前端产物与 webkit；只有含 `echo-app` 的分片才安装 node/pnpm 并构建前端。这本身就是 setup 时间的一部分收益。

### 5.3 Linux 单 job 的步骤顺序（形态稳定性）

```text
Format check
Task 16.1                                  # cargo check -p echo-app --locked
Task 9.7 9.8
Clippy                                     # check 形态，必须在 Test 之前
Test                                       # cargo test --workspace --all-targets --all-features
Rust static gates                          # --sections=rust-static（--list 与 Test 同形态）
```

最后一条 cargo 调用（两个 validator 的 `cargo test -p <pkg> --all-targets --all-features -- --list`）与 Test 同形态，因此这个 job 保存的缓存永远是「完整 build 形态」，下一个 run 的 Test 直接可用。两个 validator 当前已经带 `--all-targets`，热构建下实测 0.9s。

## 6. 详细决策

### D1：缓存治理（R1）

- 所有 `Swatinem/rust-cache` 步骤统一加：
  - `shared-key: <见 5.1 缓存角色>`（矩阵 job 用 `${{ matrix.cache }}`）；
  - `save-if: ${{ github.ref == 'refs/heads/main' }}`。
- 写缓存的 job 只有 6 个：`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`。其余 job 用相同的 `shared-key` 但 `save-if: false`，只读。
- 预估总量：1.3 + 0.9 + 1.0 + 0.6 + 0.7 + 0.3 ≈ **4.8 GB**，为 10 GB 上限留一半余量。
- 实施时先删除现存 14 条旧缓存（`gh api -X DELETE`），避免旧配额继续触发淘汰。
- 这是一次性操作，写进 change 的验证步骤。

### D2：clippy 与 test 分形态（R2）

不新增 job（并发预算不够），改为**同一 job 内 clippy 在 Test 之前**，并保证 Test 是该 job 里最后一次全量编译。这样：

- clippy 的 check 产物被 Test 的 build 覆盖，缓存里留下的是 build 形态；
- 三平台 clippy 仍然在 PR 上执行（满足规格「PR 必须跑三平台 fmt/clippy」）。

### D3：平台测试分片（R3）

- Windows / macOS 的完整 workspace 测试按包分两片：`core`（`-p echo-core`）与 `desktop`（`-p echo-sync -p echo-desktop -p echo-app`）。测试数 1101 : 804，比例接近 58:42，与实测执行时间（358s 中 core 约占 208s）一致。
- Linux 不分片：它需要保持 `--workspace` 形态给 validator 复用，且热缓存下只有约 440s。
- `Test` 步骤的 `if` 从「PR 只 Linux」改为按分片：
  - `workspace`：始终执行；
  - `core`：`github.event_name != 'pull_request' || matrix.shard == 'workspace'`；
  - `desktop`：job 级 `if: github.event_name != 'pull_request'`。

### D4：收敛脚本内的 cargo 形态（R4）

| 位置 | 现状 | 改为 |
|---|---|---|
| `run-scenario.mjs` 的场景 cargo 命令 | `cargo test -p <pkg> --all-features`（3 条） | 归一化为一条 `cargo test --workspace --all-targets --all-features`（带额外参数的命令不归一化） |
| `task-9.1.mjs` | `cargo check -p echo-app --locked`、`cargo test -p echo-desktop --lib --locked runtime::` | 加 `--all-features`；`--lib` 保留（它断言的是 lib 内测试名） |
| `task-16.1.mjs` | `cargo check -p echo-app --locked` | 加 `--all-features` |
| `task-13.8.mjs` | `cargo test -p echo-desktop --all-features <filter>` | 保留；热缓存下应为数秒，实测再定 |
| `task-12.4.mjs` | 三次独立 vitest | 合并成一次多文件 vitest 调用 |
| `injection-proofs` 的 shell 构建 | `cargo build -p echo-app` | `cargo build -p echo-app --all-features`（与 workspace 测试的 bin 单元同形态，吃缓存） |

场景命令归一化在 runner 层做（`groupedTestCommand`），注册表里保留按包的原始命令，语义不变；`validate-scenario-commands` 的语法校验也保持不变。

### D5：Release gate 只需一次编译（R4）

归一化后 29 条命令去重为：1 条 workspace 测试 + 1 条 vitest + 23 条 node 检查 + 少量带额外参数的 cargo 命令。预计 787s → ~310s。

`ECHO_VERIFY_TEST_CONCURRENCY` 保持 2：归一化后只剩一条 cargo 测试命令，提高并发没有收益，而 node 检查会改文件、必须串行。

### D6：注入证明 2 → 3 分片（R3）

- 注入证明总耗时 975s（506 + 469），三片约 325s/片。
- 嵌入前端条目仍固定在构建 shell 的分片（shard 1）。
- 分片按实测成本显式标注 `shard:`，不靠位置轮转，避免某片被重证明压到 600s 以上。
- `injection-governance.mjs` 与 `task-1.5.mjs` 同步从 `[1, 2]` 改为 `[1, 2, 3]`。

### D7：Task gates 4 → 3 分片

`frontend-a`（146s）与 `frontend-b`（296s）合并为 `frontend`（~442s），保留 `core`（302s）与 `desktop`（360s）。三个 shard 都低于 600s，省下的实例用于 Rust 分片。

### D8：Governance static 吸收 purity 与 dependency audit

- 新增 `--sections=purity` 步骤（`ECHO_PURITY_REQUIRE_BUILD=1`），放在 coverage 之前。
- `dependency-audit` job 并入，运行 `pnpm verify:task -- 1.7`。
- 预计 338s → ~380s（`cargo install` 命中缓存后回落到数十秒）。
- 这样 `purity` 不再占用 Rust (Linux) 的关键路径，同时保住 job 预算。

## 7. 失败率

1. **缓存命中（R1）**：预计消除「超时 / 5 秒墙钟断言 / 冷编译抖动」这一整类失败。这是 19 + 18 + 9 次失败的主要同源因素。
2. **依赖审计分层**：`task-1.7` 的在线公告扫描改为 **main + 每日定时**执行，PR 只跑离线半（`cargo deny check licenses bans sources` + `deny.toml`/`audit` 忽略清单镜像）。理由：新 RUSTSEC 公告与 PR 改动无关，却会同时阻塞所有在途 PR；main 与定时任务仍会真实执行并暴露。跳过时必须打印显式 note，符合规格「门禁不得静默通过」。
3. **注入证明耦合**：分片后单片负载下降；`injection-governance.mjs` 的 ci-structure 证明随新结构同步重写，并在 `task-1.5` 增加「删掉一个 matrix 分片必须失败」的证明。
4. **Release gate 证据缺失**：归一化后命令集合从 29 条降到 26 条，输出合并为单一 workspace 测试日志，`evidence not observed` 的定位信息不变（仍打印 scenario id + target）。改造后重新统计，若仍高发再单独定位。

## 8. 与 2026-11-02 设计的差异

用户选择的是「B 方案」，但 B 的两项内容经查与既有约束冲突，本次调整如下：

| B 方案原内容 | 本次处理 | 原因 |
|---|---|---|
| 前端矩阵 3 → 1（Linux） | **不做** | `engineering-governance` 规格要求 CI 矩阵为每个受支持平台提供平台语义标识；前序设计也明确「不把前端矩阵缩成单平台」。改动它需要先改规格，属于覆盖收缩。 |
| clippy 从 Rust 矩阵拆成独立 job | **改为同 job 内前置** | 拆成独立 job 会增加 3 个实例（23 > 20 并发上限），排队成本大于收益。 |
| Windows platform Gate 的 native 测试并入 Rust (Windows) | **不做** | 并入后该分片约 640s，超过 10 分钟目标。 |
| dependency-audit 用预编译二进制 | **改为缓存 + 并入 governance-static** | 仓库现有注释明确拒绝 `taiki-e/install-action`，理由是供应链只走 crates.io；继续用 `cargo install --locked`，靠缓存消除重复编译。 |

除此之外 B 的目标（Windows/macOS 测试分片、注入分片、release gate 压缩、形态收敛）全部保留。

## 9. 治理门同步改造

CI 形状被结构门禁反向锁定，以下文件必须与 `ci.yml` 同一次改动：

| 文件 | 改动 |
|---|---|
| `scripts/verify/checks/task-1.5.mjs` | 新的 5 项 `rust` 矩阵断言（platform/shard/os/packages/clippy 全量 deep-equal）；fmt/clippy 位置；`Test` 的 `if` 与形态；16.1/9.7-9.8 位置；task-gates 3 shard 与 42 项并集；injection `[1,2,3]`；section 映射（`purity` → governance-static）；`assertAlwaysRuns` 覆盖新 job id |
| `scripts/verify/injection-governance.mjs` | ci-structure 注入证明按新结构重写：删除 matrix 分片、把 clippy 移到 Test 之后、把 `purity` 从 governance-static 删除、把 injection 分片数改回 2，都必须让 `task-1.5` 失败 |
| `scripts/verify/run-scenario.mjs` | 场景 cargo 命令归一化 |
| `scripts/verify/checks/task-9.1.mjs`、`task-16.1.mjs`、`task-12.4.mjs` | 形态对齐（见 D4） |
| `scripts/verify/check-scale.mjs` | `ci.yml` 会超过 526 行；确认仍在 1000 行上限内，超出则将重复的 setup 步骤抽成 YAML anchor 或复用 composite action |
| `package.json` | 无新增脚本 |

`check-scenario-evidence.mjs` 的 `FLOORS`（523 场景 / 541 证据）与 `DELEGATED_GATES`（governance、perf）不变。

## 10. 风险

| 风险 | 影响 | 缓解 / 回退 |
|---|---|---|
| 缓存仍然不命中（配额被其他 key 占满） | 全部估算失效，main 回到 15 分钟以上 | 实施第一步就删除旧缓存并把 `save-if` 收紧到 main；合并后第一天观察 `Cache restored` 日志，若仍 miss 则把 6 个缓存 key 砍到 3 个（每平台一个） |
| `shared-key` 导致并发 job 抢写同一 key，只有第一个保存成功 | 缓存内容偏向某一种形态 | 只有指定 writer 保存；reader 全部 `save-if: false` |
| 20 个实例正好等于并发上限 | 一个 job 排队就吃掉 30–100s | 目标 20 是硬上限；若实测排队明显，优先把 `perf`（320s）并入 `release-gate` 会超时——改为把 `task-gates frontend` 拆回两片会加实例，因此真正的回退是让 `macOS native Gate` 与 `macOS platform Gate` 串行合并（682s，牺牲 main 目标） |
| macOS 5 个实例等于常见 macOS 并发上限 | macOS job 排队 | macOS 相关实例恰为 5（frontend、core、desktop、platform、native）；若排队，把 `macOS native Gate` 合并进 `desktop` 分片（约 590s） |
| 分片改变了测试之间的隐式顺序依赖 | 某些测试单独跑会失败 | 本地按分片各跑一遍 `cargo test -p ... --all-targets --all-features`；main 合并后逐片确认 |
| `task-1.5` 的结构断言与 CI 文本强耦合 | 以后改 CI 要同步改门禁 | 这是仓库既有纪律；本次一次性把断言写成按 matrix entry 的 deep-equal，失败信息精确到字段 |
| 依赖审计分层被理解为放水 | 评审争议 | 规格要求「不得静默通过」：PR 上打印显式 note，main 与每日定时仍真实执行；在 delta spec 中写明分层 |

## 11. 验证与验收

### 本地验证（实现阶段）

1. `node scripts/verify/checks/task-1.5.mjs`：新矩阵、分片、section 映射全部通过。
2. `node scripts/verify/injection-suite.mjs --shard=1/3`、`2/3`、`3/3`：三片各自通过，完整性检查覆盖全集。
3. `node scripts/verify/run-scenario.mjs -- --automated`：归一化后场景与证据数不变（561/561）。
4. `node scripts/verify/check-scenario-evidence.mjs`、`check-scale.mjs`：通过。
5. 在 `cargo test --workspace --all-targets --all-features` 之后运行两个 validator：合计 < 30s。
6. 独占运行 `pnpm verify:governance`：全绿（5 秒墙钟断言要求不并行跑 cargo）。
7. 分片各自独立跑一遍测试，确认无隐式顺序依赖。

### CI 验证

1. 分支 PR：记录总墙钟与每个 job 墙钟，确认 < 10 分钟。
2. 合并后 main：确认 5 个 rust 分片全部执行（`desktop` 分片不得是 skipped），总墙钟 < 10 分钟。
3. 连续两次 main run 比较：第二次应出现 `Cache restored successfully` 且 Test 步骤编译数显著下降（目标 < 50 个 crate）。
4. `gh api repos/zsxink/Echo/actions/cache/usage`：总量应 < 6 GB。
5. 记录失败率基线对比（改造后 30 天）。

### 验收标准

- PR 与 main 的代表性 run 墙钟均 < 10 分钟（连续两次 run 取第二次，避开冷缓存）。
- 523 场景 / 541 证据、注入证明条目数、42 项 task 清单不减少。
- 三平台 fmt/clippy 在 PR 上执行；Windows/macOS 完整 workspace 测试在 main 上执行（分片后仍覆盖全部包）。
- 缓存总量 < 6 GB，且 main run 的 Rust job 日志出现主键命中。
- `pnpm verify:governance` 本地全绿。

### 回退顺序

1. 若 Linux 关键路径 > 560s：把 `Rust static gates`（validator）移到 Release gate 的场景步骤之后。
2. 若 main 仍 > 10 分钟且瓶颈是 Windows：把 `desktop` 分片再拆成 `desktop` 与 `app`（实例 +1，需要从 `task-gates frontend` 借一个）。
3. 若缓存仍然不命中：把 6 个 shared key 收敛为 3 个（每平台一个），接受 reader job 的部分重建。
