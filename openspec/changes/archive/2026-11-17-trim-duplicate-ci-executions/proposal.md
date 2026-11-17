# Proposal

## Why

Issue #156 把 CI 的关键路径压到 Release gate，并消除了 `rust-static` 约 5 分钟的重编。但同一次 run 里仍有多处**同一检查被执行两遍**，以及一个只跑 3 个场景却独占实例的 job。这些重复既消耗 Actions 配额（#156 的原始诉求），也扩大失败面：任一份重复执行 flake，整条 run 就红。

按 run `44008849452`（冷）与热 run 的分批耗时实测，本次删除的是四类**确证重复**，不是「看起来可以省」的检查。

## What Changes

- **`injection-proofs` 的 shard 3 不再构建前端**。只有 shard 1（embedded-frontend 证明比对 `dist`）与 shard 2（`task-9.1` 的 `cargo check -p echo-app` 缺 `dist` 直接 FAIL）需要它；实测 shard 3 无 `dist` 时 19.1s 通过。原注释「no other shard needs either」与事实不符，一并修正。
- **`rust-desktop` 删除重复的 `Format check`**。`rust` job 的 macOS / Windows `core` 分片已在同一平台、同一 checkout 上跑同一条 `cargo fmt --all -- --check`。
- **`task-13.2` 删除裸跑的 `player_smoke`**。同一 job 里的 `task-8.12` 用 `DYLD_LIBRARY_PATH=target/Frameworks` 跑同一 suite，并在命中 skip 分支时 FAIL；13.2 裸跑拿不到 rpath，会静默 skip，却在报告里宣称「player_smoke (real libmpv)」。删除的是**更弱的一份**，覆盖不降反升。
- **`frontend` 矩阵由 3 平台收敛为 Linux 单腿**。57 个前端测试文件中 0 个读取 `process.platform` / `win32` / `darwin`；检查全部是 jsdom + Node 工具链，与宿主平台无关。macOS / Windows 的前端**构建**覆盖仍由 `rust` / `rust-desktop` 的 `Build frontend for Tauri context` 保留。
- **`perf` job 归并进 `task-gates` core 分片**。`perf` 只跑 3 个委托场景，独立占一个实例。归并后不再单独占实例；`task-gates` 的缓存步骤是 `save-if: false`，`--release` 产物不会污染 `linux-workspace`。

非目标：

- **不删除 Rust Linux 的 `Test` 步骤**（原「workspace 测试只跑一次」方案）。`rust-static` 的测试清单来自 `cargo test --workspace --all-targets --all-features --no-run`；删掉 `Test` 后这批二进制只能由 `--no-run` 现编，编译成本原样回来。实测账：1085s − 334s（rust-static 修复）− 359s（Test 执行）+ ≈300s（现编）≈ 692s，**净省约 60s**，却要拆掉缓存形态不变量的锚点、改写 spec 第 374/378 行与 4 条注入证明。成本远大于收益，且提高失败率。
- **不改 `task-9.1` / `task-13.8` 的 cargo 形态**。它们与已修复的 rust-static 属同一特性统一根因，是 Release gate 关键路径上下一个约 180s（热态）的杠杆，但属新范围。
- 不降低任何检查的断言强度；不改变产品行为、IPC 契约、数据库结构或发布流程。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`：修改「CI status checks use stable platform names」——前端检查收敛为 Linux 单腿后，显示名仍 MUST 保持平台语义（`Frontend (Linux)`），且 MUST NOT 因收敛而退化为无平台标识的名字；修改「CI 执行分层与结构完整性」——新增「同一检查 MUST NOT 在多个 job 中重复执行」的约束与对应结构断言，并把性能场景的执行位置纳入结构校验。

## Impact

- CI：`.github/workflows/ci.yml`（`injection-proofs` 前端构建条件、`rust-desktop` 步骤、`frontend` 矩阵、`perf` job 删除与 `task-gates` 步骤新增）。
- 验证脚本：`scripts/verify/checks/task-1.5.mjs`（frontend 矩阵断言、`DESKTOP_ORDER`、perf 断言改写、新增重复执行断言）、`scripts/verify/checks/task-13.2.mjs`、`scripts/verify/injection-governance.mjs` 与 `injection-shard-plan.mjs`（受影响的注入证明改写 + 新增）。
- 文档：`docs/arch-standards-remaining.md` §11 的 CI 拓扑表。
- 兼容性：无产品代码、无 IPC/DTO、无数据库与同步载荷变化；`Frontend (Linux)` 检查名保持平台语义。
- 关联 Issue：#158。
