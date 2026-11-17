# Design

## Context

- 详见 `docs/superpowers/specs/2026-11-16-ci-runtime-compression-design.md`（已批准）与 `docs/superpowers/plans/2026-11-17-ci-runtime-under-10-min.md`。本文件记录落地该设计的技术决策与取舍，动机见 `proposal.md`。
- 当前 `ci.yml` 的 18 个 job 各自保存 0.2–1.3 GB 的 Rust target 缓存，总量 10.13 GB 超过 GitHub Free 的 10 GB 上限，导致最需要缓存的 `rust` 三平台 job 拿不到缓存；`Test` 步骤在 `clippy` 之后仍冷编译 318–413 个 crate。
- CI 形状被结构门禁反向锁定：`scripts/verify/checks/task-1.5.mjs` 断言 `ci.yml` 的矩阵、job 与步骤顺序，`scripts/verify/injection-governance.mjs` 对结构门禁做注入证明。workflow 与门禁 MUST 在同一次改动内同步，否则治理门禁先红。
- 本 change 只改 CI workflow 与验证脚本，不触碰生产代码、Core/IPC 契约、数据库或同步载荷；分层与依赖方向保持 `scripts/verify/**` 依赖仓库文本、CI 依赖脚本的既有结构，不新增跨层依赖。

## Goals / Non-Goals

**Goals:**

- PR 与 `main` 的代表性 run 墙钟均 < 10 分钟（连续两次 run 取第二次，避开冷缓存）。
- 缓存总量压到 10 GB 配额的一半左右，并让 `main` 的 Rust job 命中主键。
- 降低失败率：缓存命中消除超时/墙钟抖动类失败，依赖审计分层消除与 PR 无关的 RUSTSEC 阻塞，注入证明分片降低单片负载。
- 不删除检查、不降低断言强度、不缩小平台覆盖；job 实例数 ≤ 20，macOS 实例 ≤ 5。

**Non-Goals:**

- 不改变产品行为、IPC/DTO、数据库、同步与发布产物。
- 不把前端三平台矩阵缩成单平台；不把 Windows platform Gate 的原生测试并入 Rust 分片。
- 不引入 `taiki-e/install-action` 等新的第三方供应链入口，也不新增 `package.json` 脚本。

## Decisions

### D1. 缓存治理：固定 shared-key，写者收敛为 6 个且只写 main

- 所有 `Swatinem/rust-cache` 统一 `shared-key`：`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`；矩阵 job 用 `${{ matrix.cache }}`。
- 写者集合是 6 个 key，由 `rust` 与 `rust-desktop`（两者均经 `${{ matrix.cache }}` 展开，合计 5 个 key）以及 `governance-static` 写入，`save-if: ${{ github.ref == 'refs/heads/main' }}`；其余 job 使用相同 `shared-key` 但 `save-if: false`，只读。
- 实施第一步删除现存 14 条旧缓存，避免旧配额继续触发淘汰。
- 选择按分片而非每平台单 key：分片形态不同，共享一个 key 会让缓存内容偏向某一种形态；6 个 key 的预估总量约 4.8 GB，为配额留一半余量。

### D2. clippy 在 Test 之前，缓存内容固定为 build 形态

- 不新增 job（20 实例已是并发上限），改为同一 job 内 `clippy` 前置、`Test` 作为最后一次全量编译；clippy 的 check 产物被 test 的 build 覆盖，缓存中留下 build 形态。
- 三平台 clippy 仍在 PR 上执行，满足规格「PR 必须跑三平台 fmt/clippy」。
- 写入共享缓存的 `rust` 矩阵分片，其最后一条构建型 cargo 调用 MUST 与缓存形态一致（Linux 为 `cargo test --workspace --all-targets --all-features`，core/desktop 分片为 `cargo test ${{ matrix.packages }} --all-targets --all-features`）；其后只允许同形态的 `-- --list`。`governance-static` 的缓存 key 是 job 私有的（仅该 job 与定时依赖审计 workflow 读取），其 coverage 插桩形态不会污染任何其他读者，因此不受该形态约束；缓存写入只发生在 `main` 的规则仍适用于全部写者。

### D3. Windows/macOS workspace 测试按包分片，Linux 保持 workspace

- Windows / macOS 分 `core`（`-p echo-core`）与 `desktop`（`-p echo-sync -p echo-desktop -p echo-app`）两片；测试数约 1101 : 804，与实测执行时间比例一致。
- `desktop` 分片在 `pull_request` 上整体跳过，在 `main` 上必须真实执行；分片并集覆盖全部 workspace 包，MUST NOT 因分片删包。
- 分片落在两个 job 里：`rust` 承载 Linux `workspace`、macOS `core`、Windows `core`；`rust-desktop` 承载 macOS `desktop`、Windows `desktop`，其 job 级 `if` 只在非 `pull_request` 成立。job 级 `if` 在矩阵展开前求值、看不到 `matrix.*`，所以需要按事件跳过的分片不能与按分片条件执行的步骤共用同一个矩阵。
- Linux 不分片：它需要保持 `--workspace` 形态供 validator 复用，且热缓存下约 440s。
- **偏差（已评估）**：`core` 分片仍构建前端并执行 `cargo clippy --workspace`。若改成 `-p echo-core`，macOS/Windows 的 clippy 会被收窄到 core，而这些平台的 `#[cfg]` 代码主要在 shell 与播放器 crate 中，属于覆盖收缩。代价是两个 core 分片各多约 70s setup，仍低于关键路径。

### D4. 依赖审计分层与独立定时 workflow

- `dependency-audit` job 并入 `governance-static`；在线 RUSTSEC 公告扫描只在 `main` 与每日定时任务真实执行，PR 只跑离线半（`cargo deny check licenses bans sources` + `deny.toml`/`audit` 忽略清单镜像）。
- 跳过在线半时 MUST 打印显式 note，并由结构门禁断言执行条件与 note 存在性，避免分层退化为永久静默。
- **偏差（已评估）**：每日定时放入新 workflow `.github/workflows/dependency-audit.yml`（仅 `schedule` + `workflow_dispatch`，1 个 job），而不是把 `schedule` 加进 `ci.yml`——后者会让整条 20 实例流水线每天空跑一次，与压缩目标相反。

### D5. task gates 4 → 3 片，注入证明 2 → 3 片

- task gates：`frontend-a`（146s）与 `frontend-b`（296s）合并为 `frontend`（约 442s），保留 `core`（302s）与 `desktop`（360s）；三片均低于 600s，省下的实例用于 Rust 分片。
- 注入证明：总耗时 975s（506 + 469），拆为 3 片约 325s/片；嵌入前端条目固定在构建 shell 的分片，分片按实测成本显式标注 `shard:`，不靠位置轮转。
- `injection-governance.mjs` 与 `task-1.5.mjs` 的分片集合同步从 `[1, 2]` 改为 `[1, 2, 3]`；注入分片计划放入新文件，因为 `injection-suite.mjs` 已 997 行、`check-scale.mjs` 上限 1000 行。

### D6. cargo 形态收敛

| 位置 | 现状 | 改为 |
|---|---|---|
| `run-scenario.mjs` 场景 cargo 命令 | `cargo test -p <pkg> --all-features` | 归一化为 `cargo test --workspace --all-targets --all-features`（带额外参数的命令不归一化） |
| `task-9.1.mjs` | `cargo check -p echo-app --locked`、`cargo test -p echo-desktop --lib --locked` | 加 `--all-features`；`--lib` 保留（断言的是 lib 内测试名） |
| `task-16.1.mjs` | `cargo check -p echo-app --locked` | 加 `--all-features` |
| `task-13.8.mjs` | `cargo test -p echo-desktop --all-features <filter>` | 保留，热缓存下应为数秒 |
| `task-12.4.mjs` | 三次独立 vitest | 合并为一次多文件 vitest 调用 |
| `injection-proofs` shell 构建 | `cargo build -p echo-app` | `cargo build -p echo-app --all-features` |

场景命令归一化在 runner 层（`groupedTestCommand`）做，注册表保留按包的原始命令，语义不变；`validate-scenario-commands` 的语法校验保持不变。

### D7. 结构门禁按新矩阵重写并自证

- `task-1.5.mjs` 对 `rust`（3 项：Linux `workspace`、macOS `core`、Windows `core`）与 `rust-desktop`（2 项：macOS `desktop`、Windows `desktop`）两个矩阵分别做 platform/shard/os/packages/clippy 全量 deep-equal，断言 fmt/clippy 位置、`Test` 的 `if` 与形态、16.1/9.7-9.8 位置、`rust-desktop` 的 job 级 `if`、task-gates 3 分片与 42 项并集、injection `[1,2,3]`、section 映射（`purity` → `governance-static`）与 `assertAlwaysRuns` 覆盖新 job id。
- 追加一条永久断言：任何 job 级 `if` MUST NOT 引用 `matrix.`。GitHub 的 job 级 `if` 在矩阵展开之前求值，可用上下文只有 `github` / `needs` / `vars` / `inputs`；引用 `matrix.` 会让整个 workflow 在解析期被拒，所有 job 一起消失。这也是把 `desktop` 分片拆到独立 `rust-desktop` job 的原因——job 级 `if` 看不到矩阵维度，需要按事件跳过的分片必须落在独立 job 里。
- `injection-governance.mjs` 的 ci-structure 注入证明按新结构重写：删除 matrix 分片、把 clippy 移到 Test 之后、把 `purity` 从 `governance-static` 删除、把 injection 分片数改回 2，都必须让 `task-1.5` 失败；证明总数由 12 条增至 13 条，新增 `ci-structure/job-if-references-matrix`（把 `rust-desktop` 的 job 级 `if` 改为引用 `matrix.shard` 时必须失败）。
- 选择 deep-equal 而不是逐字段包含断言：失败信息能精确到字段，避免新增/删除矩阵字段时门禁静默放行。

## Risks / Trade-offs

- **缓存仍不命中（配额被其他 key 占满）** → 实施第一步删除旧缓存并把 `save-if` 收紧到 `main`；合并后观察 `Cache restored` 日志，若仍 miss 则把 6 个 key 收敛为 3 个。
- **shared-key 并发抢写** → 只有指定 writer 保存，reader 全部 `save-if: false`。
- **20 个实例正好等于并发上限** → 排队明显时，回退方案是把 `macOS native Gate` 与 `macOS platform Gate` 串行合并（682s，牺牲 main 目标）；不回退为删除检查。
- **macOS 5 个实例等于常见并发上限** → 若排队，把 `macOS native Gate` 合并进 `desktop` 分片（约 590s）。
- **分片改变测试间的隐式顺序依赖** → 本地按分片各跑一遍 `cargo test -p ... --all-targets --all-features`，main 合并后逐片确认。
- **结构断言与 CI 文本强耦合** → 仓库既有纪律；本次把断言写成按 matrix entry 的 deep-equal，并在同一任务内同步门禁与注入证明。
- **依赖审计分层被理解为放水** → PR 打印显式 note，main 与每日定时仍真实执行，delta spec 与结构门禁同时固定。

## Migration Plan

1. 先删旧缓存，再按 D1–D7 修改 `ci.yml`、新增 `dependency-audit.yml`，同一改动内同步 `task-1.5.mjs`、`injection-governance.mjs`、`injection-suite.mjs` 与 cargo 形态相关脚本。
2. 分支 PR 验证墙钟与全部分片实际执行；合并后观察连续两次 `main` run 的缓存主键命中与 Test 编译数（目标 < 50 个 crate）。
3. 回滚：还原 workflow 与门禁脚本即可；缓存 key 变更后旧 key 可保留或删除，不影响代码与数据。

## Open Questions

无。
