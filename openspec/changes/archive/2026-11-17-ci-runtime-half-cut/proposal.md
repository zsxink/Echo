# Proposal

## Why

当前 PR run（`44037252640`）执行 15 个实例、合计 7233 runner-秒、墙钟 17m13s，其中至少 3966 秒花在**已被其他 job 覆盖的重复执行**、**只验证已归档 task 的旧门禁**和**只在本地才有意义的治理元测试**上。PR #157 已把「重复执行」写成 `engineering-governance` 的 MUST NOT，但 `cargo test --workspace --all-targets --all-features` 仍在 Rust Linux 与 Release gate 中逐字节跑两遍，287 条场景因此各被证明两次。用户要求在当前分支追加优化，消掉至少 50% 的 CI 机器时间，同时保留产品正确性检查。

## What Changes

- **删除 `injection-proofs` 的 3 个 PR job**（−1762s，−3 实例）。64 条 mutation proof 仍保留在仓库中，可随时用 `node scripts/verify/injection-suite.mjs` 本地全量运行；CI 不再为「证明门禁会失败」的元测试付出每次 PR 的编译与运行成本。
- **task gates 由 3 个分片收缩为 1 个**（−960s，−2 实例）。桌面/前端分片中的 31 条 task 全部是 `cargo test -p echo-desktop` / vitest / typecheck 的子集，已由 workspace test 与 Frontend job 覆盖；保留 4 条独有门禁（`1.1` 工作区清单、`1.3` 锁文件/自检、`12.5` 50k 性能预算、`13.3` 故障注入报告）与 `pnpm verify:perf`。`13.1`（headless Chromium E2E）迁入 Frontend job。合计归档 37 条 task（core 6 + desktop 17 + frontend 14）。
- **删除 Rust Linux 中的 `Task 16.1` 与 `Task 9.7 9.8`**（−331s）。`9.7/9.8` 是 Linux 上的空转（无 vendored libmpv），真实执行在 macOS/Windows platform Gate；`16.1` 的 `open_targets` 行为测试由 workspace test 覆盖，其独有的静态契约（单一 owner、禁止 `url.to_string()`）改为 `--static-only` 在 governance-static 中执行。
- **PR 不再启动 macOS/Windows 的 `core` 分片**（−501s，−2 实例）。两平台的完整 workspace 测试、`fmt` 与 `clippy` 合并进 main-only 的 `rust-desktop` job；PR 仍由 macOS/Windows platform Gate 编译并测试平台相关代码。这是对「PR 三平台 fmt/clippy」的显式规格修改。
- **Release gate 委托重复场景**（−433s）。287 条 `cargo test -p echo-core|echo-desktop|echo-app --all-features` 场景显式标记 `delegatedTo: "workspace-test"`，202 条 vitest 场景标记 `delegatedTo: "frontend-test"`；Release gate 通过 `--delegated=governance,perf,workspace-test,frontend-test` 跳过它们，不再重跑完整 workspace test 与完整 vitest。
- **coverage 从 PR 移到 main**（−249s）。`registry,static` 仍在 PR 上执行；`coverage` 在 main 上执行，并由结构门禁断言其执行位置与 PR 跳过条件。
- **纯文档 PR 不再触发全量 CI**。`ci.yml` 的 `push` / `pull_request` 增加 `paths-ignore`，只忽略经审计确认不是门禁输入的文档（`docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md`、`docs/interface-terminology.md`、`docs/arch-standards-remaining.md`）；`README.md`、归档 OpenSpec 证据与 `artifacts/` 被 gate 读取，明确不列入。

预计 PR 机器时间从 7233s 降到约 3000s（−58%），关键路径从 1030s 降到约 500–600s；产品正确性检查（workspace test、frontend test、三平台 platform Gate、perf、fault injection、50k 预算、E2E）全部保留。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `engineering-governance`: PR/main 分层从「PR 三平台 fmt/clippy」改为「PR Linux fmt/clippy + 平台 Gate，main 三平台 fmt/clippy/test」；task 分片只覆盖未归档且有 CI 执行位置的 task；场景委托新增 `workspace-test` / `frontend-test` 两个真实 gate；coverage 允许 PR 跳过、main 执行；injection proof 不再是 CI section，保留为本地可执行套件；纯文档 PR 的 paths 过滤纳入结构校验。

## Impact

- `.github/workflows/ci.yml`：删除 `injection-proofs`；`rust` 矩阵收敛为 Linux；`rust-desktop` 扩展为 full workspace；`task-gates` 收敛为 core 单分片；`frontend` 增加 `13.1`；`governance-static` 拆分 coverage；`macos-platform-gate` 增加 embedded-frontend 检查；`release-gate` 增加 delegation。
- `scripts/verify/checks/task-1.5.mjs`：重写矩阵、缓存写者、step 顺序、task 分片、section 映射与重复执行反向锁。
- `scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`、`scripts/verify/check-scenario-evidence.mjs`、`scripts/verify/run-scenario.mjs`：新增两类委托并生成 manifest。
- `scripts/verify/ci-governance.mjs`：移除 `injection` section，coverage 保持独立 section。
- `scripts/verify/checks/task-16.1.mjs`、`scripts/verify/manifest.json`：`16.1` 增加 `--static-only`，命令改为静态契约检查。
- `scripts/verify/injection-governance.mjs`、`scripts/verify/injection-shard-plan.mjs`：删除/重写引用已删除结构的 proof。
- `openspec/specs/engineering-governance/spec.md`：通过 delta spec 修改上述要求。
- 不影响产品代码、数据库 schema、同步协议或 UI 行为。
