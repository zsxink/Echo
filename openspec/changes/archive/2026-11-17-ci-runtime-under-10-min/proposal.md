# Proposal

## Why

Issue #156 指出：最近一次 `main` run（`43827801014`）墙钟 23m47s，最近 30 天 PR 平均约 17.5 分钟，且 159 次 run 中有 62 次至少一个 job 失败。直接根因是 Actions 缓存总量 10.13 GB 超过 GitHub Free 的 10 GB 上限，导致三平台 Rust job 在同一次 run 中全部未命中缓存，Test 步骤每次冷编译 318–413 个 crate；同一 job 内 clippy（check 形态）与 test（build 形态）互相作废，单 job 串行堆叠多条关键路径，脚本内还存在多种异形 cargo 调用。前序设计 `docs/superpowers/specs/2026-11-02-ci-pr-under-10-min-design.md` 只承诺 PR 且把 `main` 列为非目标，本次需要把 `main` 一并纳入 10 分钟目标。

## What Changes

- **缓存治理**：所有 `Swatinem/rust-cache` 步骤统一 `shared-key`，写者收敛为 6 个（`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`），且 `save-if` 只允许在 `main` 为真；实施第一步删除现存 14 条旧缓存，把总量压到 10 GB 配额的一半左右。
- **Rust 矩阵分片与步骤重排**：`rust` job 承载 Linux `workspace` 与 Windows/macOS `core` 三个分片，新增 `rust-desktop` job 承载 Windows/macOS `desktop` 两个分片；`rust-desktop` 仅在非 `pull_request` 执行。拆成两个 job 是因为 GitHub 的 job 级 `if` 在矩阵展开之前求值、看不到 `matrix.*`，需要跳过的分片必须落在独立 job 里。`clippy` 移到 `Test` 之前，保证每个 rust 分片缓存 key 的最后一次构建型 cargo 调用与缓存形态一致；`purity` 从 `Rust (Linux)` 迁入 `governance-static`；新增 `macos-native-gate` job，承接从 rust job 迁出的 8.12/13.2 libmpv smoke 与 native e2e 门禁。
- **依赖审计分层**：`dependency-audit` job 并入 `governance-static`；在线 RUSTSEC 公告扫描只在 `main` 与新增的每日定时 workflow 真实执行，PR 只跑离线半，并在跳过时打印显式 note。
- **task 与注入证明分片**：task gates 由 4 片收敛为 3 片（`frontend-a` + `frontend-b` 合并为 `frontend`）；注入证明由 2 片拆为 3 片，并同步 `injection-governance.mjs` 与 `task-1.5.mjs`。
- **cargo 形态收敛**：场景命令在 runner 层归一化为 `cargo test --workspace --all-targets --all-features`；`task-9.1`、`task-16.1`、`task-13.8`、`task-12.4` 与注入证明 shell 构建对齐到同一 build 形态。
- **结构门禁同步**：`task-1.5.mjs` 的 CI 结构断言按 `rust` 3 项 + `rust-desktop` 2 项两个矩阵重写，并新增「任何 job 级 `if` 不得引用 `matrix.`」的永久断言；`injection-governance.mjs` 增加“删掉 matrix 分片、移动 clippy、删除 purity、改回 2 片注入、job 级 `if` 引用 `matrix.`”必须失败的注入证明（证明总数 13 条）。

非目标：

- 不删除任何检查、不降低断言强度、不缩小平台覆盖（三平台前端矩阵、三平台 `fmt`/`clippy`、各平台 Gate 均保留）。
- 不把前端矩阵缩成单平台；不把 Windows platform Gate 的原生测试并入 Rust 分片。
- 不改变产品行为、Core/IPC 契约、数据库结构或发布流程。
- 不新增 `package.json` 脚本；不引入 `taiki-e/install-action` 等新的第三方供应链入口。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`: 修改「CI 执行分层与结构完整性」，允许 Windows/macOS 全量 workspace 测试按包分片（并集 MUST 覆盖全部 workspace 包），要求 `clippy` 在三平台以 workspace 形态执行、写入共享缓存的 `rust` 矩阵分片其最后一条 cargo 调用与缓存形态一致（`governance-static` 的 job 私有 key 豁免）、缓存写入只发生在 `main`；修改「门禁不得静默通过」，允许依赖审计的在线半在 PR 上跳过，但 MUST 在 `main` 与定时任务上真实执行、跳过时 MUST 打印显式 note，并由结构门禁断言该分层。

## Impact

- 事实来源：`docs/superpowers/specs/2026-11-16-ci-runtime-compression-design.md`（已批准）、`docs/superpowers/plans/2026-11-17-ci-runtime-under-10-min.md`、`openspec/specs/engineering-governance/spec.md`、`openspec/CODE_STANDARDS.md`。
- CI：`.github/workflows/ci.yml`（矩阵、job 拆分、缓存、步骤顺序、分片映射）；新增 `.github/workflows/dependency-audit.yml`（仅 `schedule` + `workflow_dispatch`）。
- 验证脚本：`scripts/verify/checks/task-1.5.mjs`、`scripts/verify/injection-governance.mjs`、`scripts/verify/injection-suite.mjs`（分片计划放新文件以满足 1000 行规模上限）、`scripts/verify/run-scenario.mjs`、`scripts/verify/checks/task-9.1.mjs`、`task-16.1.mjs`、`task-12.4.mjs`。
- 治理约束：`ci.yml` 的结构由 `task-1.5` / `injection-governance` 反向锁定，workflow 与结构门禁 MUST 在同一次改动内同步；`check-scale.mjs` 的 `.yml` 不在扫描扩展名内，`ci.yml` 不受 1000 行门禁约束。
- 兼容性：无产品代码、无 IPC/DTO、无数据库与同步载荷变化；CI 检查名保持平台语义（`Rust (Linux workspace)` 等），不泄露 runner label。
- 关联 Issue：#156（本 change 的实现与 PR 必须关联）。
