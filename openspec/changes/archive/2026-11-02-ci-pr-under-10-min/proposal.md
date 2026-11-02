# Proposal

## Why

PR CI 墙钟已涨到 **21m46s**（run `41323932057`），主要耗时不是断言本身，而是重复执行与串行排队：

| Job | 墙钟 | 主要问题 |
|---|---:|---|
| Task gates | 1304s | 43 个 task 在同一 job 串行，部分 task 自带 cargo 编译 |
| Governance | 1015s | 与 Rust Linux / Release gate 重复跑 fmt / clippy / test / 场景校验 |
| Rust (Windows / macOS) | 983s / 830s | PR 上执行完整 `cargo test --workspace` |
| Release gate | 827s | 静态预检 + 场景 + 3 条性能场景串行 |
| Injection proofs | 598s | 单 job |
| Rust (Linux) | 420s | `cargo test --list` 未带 `--all-targets`，重复构建 doc-test harness（323s） |

同时，现有 CI 形状被 `task-1.5`、`task-crp`、注入证明等脚本反向锁定，任何优化都必须同步扩展这些结构门禁，否则下一次改动就可能把检查静默删掉。

## What Changes

- **Task gates 分片**：43 个 task 拆为 4 个显式 shard（core 10 / desktop 17 / frontend-a 8 / frontend-b 7）；`16.1` 移到 Rust Linux。`task-1.5` 断言 4 个 shard 并集为 42 项、与 Rust Linux 的 `16.1` 合起来覆盖 43 项且无重复。
- **Governance 分段与迁移**：`ci-governance.mjs` 增加 `--sections` / `--list-sections`，按环境拆为 `quality`、`registry`、`rust-static`、`static`、`coverage`、`purity`、`embedded`、`injection`；`rust-static` / `purity` 迁到 Rust Linux 热构建之后，其余静态段留在轻量 job。
- **Release gate 只跑场景**：使用 `--skip-static --delegated=governance,perf`；3 条性能场景独立成 `perf` job；`run-scenario.mjs` 支持逗号分隔的多 gate 委托。
- **注入证明分片**：拆为 2 个 shard，新增并集 / 互斥校验；两个 shard 都安装 Rust / webkit / pnpm 依赖，shard 2 跳过 shell 构建，嵌入前端条目固定 shard 1。
- **Linux platform Gate 并入 Rust Linux**：步骤顺序为 `fmt → clippy → Test → Rust static gates → Task 16.1 → Task 9.7 9.8`。
- **平台分层**：PR 保留三平台 fmt / clippy 与平台 Gate；Windows / macOS 的完整 `cargo test --workspace` 移到推送到 `main` 时执行。Linux 在 PR 上继续跑全量测试。
- **脚本级修复**：`validate-scenario-commands.mjs` 与 `check-scenario-evidence.mjs` 的 `cargo test --list` 统一加 `--all-targets`；`task-13.9.mjs` 增加 `--skip-static`；`self-test.mjs` 使用 fake PATH；`task-1.5.mjs` 增加步骤级结构、section 覆盖与 task 分片断言。
- **不减少任何检查 / 场景 / 证据**：523 场景、541 证据、注入证明数量、43 个 task 全部保留。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `engineering-governance`：新增「CI 执行分层与结构完整性」要求——PR 与 main 的覆盖分工必须使任何检查不同时消失；task 分片并集、governance section 覆盖、场景委托目标都必须由结构门禁静态校验。同时修改「门禁不得静默通过」：显式委托 MUST 指向在 CI 中真实执行的 gate，不得成为跳过检查的通道。

## Impact

**受影响工作流**：`.github/workflows/ci.yml`（job 结构、矩阵条件、步骤顺序、injection shard、perf job、release gate 参数）。

**受影响脚本**：`scripts/verify/{ci-governance.mjs, run-scenario.mjs, validate-scenario-commands.mjs, check-scenario-evidence.mjs, scenario-commands.mjs, manifest.json, self-test.mjs, injection-suite.mjs}`、新增 `scripts/verify/injection-shard.mjs`、`scripts/verify/checks/{task-1.5.mjs, task-13.9.mjs}`、`package.json`。

**受影响文档**：`docs/arch-standards-remaining.md`、`docs/ci-pipeline-review-2026-10-28.md`、`docs/native-attestation-playbook.md`（委托与性能场景说明）、`docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md`（设计依据）。

**不变式**：

- 不修改任何产品代码，不改变既有 spec 的 WHEN / THEN 语义。
- 不放宽 `-D warnings`、coverage 阈值 90%、scale 基线、构建纯净性门。
- 不引入 nextest 等新工具链。
- `main` 仍执行三平台完整测试；PR 上的 Windows / macOS 回归由平台 Gate、clippy 与 main 兜底。

**范围外**：前端三平台矩阵收敛、dependency-audit 合并、`task-1.5` 之外的 workflow 重构。
