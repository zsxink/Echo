# Proposal

## Why

Issue #160 记录了一次全量瘦身盘点：前序 change（#156 / #158 / #159）已把 47 个与 workspace/frontend 套件重复的 task gate 标记为 `archived: true`，`run-task --all` 也会跳过它们，但对应的门禁脚本文件仍留在工作树里；其中 41 个脚本已确认没有任何执行引用。同时仓库本地还堆积了构建产物、Vite/Vitest 临时文件与一次性草稿脚本。这些"已死但未删"的治理资产持续增加清单维护成本、干扰规模扫描与代码阅读，且容易被误认为仍然生效的检查。本次把这部分低风险残留一次清干净，不改变 CI 实际执行的检查集合。

## What Changes

- 删除 41 个"仅被 `archived: true` 条目引用、无任何执行引用"的门禁脚本（`scripts/verify/checks/task-*.mjs`），合计 2,381 行 / 96 KB。
- 保留 `task-13.2.mjs`：它同属 archived 集合，但被 `scripts/verify/injection-governance.mjs` 作为防回归 guard 引用，删除会破坏注入治理套件，属于必须登记的例外。
- 同步治理清单：`scripts/verify/manifest.json` 中对应 archived 条目移除 `commands` 或整条删除，并同步 `.github/workflows/ci.yml` 关于旧 task 清单的注释与 `run-task.mjs` 的 skip 打印，使三者对 archived 的处理一致、无悬空路径。
- 清理本地被 `.gitignore` 覆盖的临时产物：`apps/desktop/dist/`、`node_modules/.vite-temp/`、`node_modules/.tmp/`、`vitest.config.ts.timestamp-*.mjs`、`benchmark-50k.json`。
- 清理 `artifacts/` 下的 42 个本地草稿（一次性重构脚本、场景快照、日志与临时报告，约 1 MB）；保留其中 2 个被 git 跟踪的验收资产（`fault-injection-13.3.md`、`native-e2e-13.2.txt`）。
- 新增可复跑的引用检查，证明待删脚本确无执行引用、保留例外确有消费者，并锁定清理结果不被回退。

非目标：

- 不修改任何业务源码、Core/IPC/DTO 契约、数据库结构或同步载荷。`cargo check --workspace --all-targets` 当前零警告，业务代码无本次可清理的死代码。
- 不改变 CI 实际执行的检查集合、平台覆盖或关键路径；不删除、不降低任何仍在执行的门禁强度。
- 不做中风险项：不将 114 MB 的 `libmpv-2.dll` 外置、不删减 `openspec/changes/archive/` 的 20 MB 归档文档、不把 140 条 task 清单整体收敛为活跃集。
- 不删除 `legacy_*` v1 兼容性契约测试、`echo-sync` crate 或前端测试文件（已核实均有真实消费者）。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`: 新增「已归档门禁与本地临时产物的清理」要求，规定 archived task gate 的脚本删除条件、仍被结构门禁引用时的例外登记，以及本地临时产物 MUST NOT 作为交付资产登记；其验收由引用检查与治理门禁证明。

## Impact

- 事实来源：`openspec/specs/engineering-governance/spec.md`（「代码变更的规格复核与残留清理」「门禁不得静默通过」「CI 执行分层与结构完整性」）、`openspec/CODE_STANDARDS.md` §9.1（规格复核与残留清理门禁）。本次不涉及 `docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md` 与界面基准。
- 治理脚本：`scripts/verify/manifest.json`、`scripts/verify/run-task.mjs`、`scripts/verify/injection-governance.mjs`（仅核对/增补引用检查，不改其断言语义）；删除 `scripts/verify/checks/task-*.mjs` 共 41 个文件。
- CI：`.github/workflows/ci.yml` 仅同步注释与 archived 描述，不改 job / step / 矩阵结构，不动由 `task-1.5.mjs` 锁定的结构断言。
- 本地文件：`artifacts/` 的未跟踪草稿、`apps/desktop` 下被忽略的构建与临时文件；不改变任何被 git 跟踪的验收资产。
- 兼容性：无产品行为、无 API / DTO / schema / 同步载荷变化；CI 执行集合不变。
- 关联 Issue：#160（本 change 的实现与 PR 必须关联）。
