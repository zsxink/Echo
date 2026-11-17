# Design

## Context

- 动机与删除清单见 `proposal.md`；关联 Issue #158，落地在现有 PR #157 的分支上。
- 上一轮（`archive/2026-11-17-ci-runtime-under-10-min`）把 CI 关键路径压到 Release gate。按 run `44008849452`（冷）实测：Release gate 1426s，Rust（Linux workspace）1085s，其余 job 全部低于 800s。热态下 Release gate 步骤 594s + 约 90s setup ≈ 11m24s，仍略超 10 分钟目标。
- **本轮四项删除都不在关键路径上**，所以它们买的是配额与失败面，不是墙钟。这一点必须写进 PR 正文，避免把「删了重复」误读成「达标了」。缩短关键路径的下一根杠杆是 `task-9.1`（82s 热）与 `task-13.8`（101s 热）在 release gate 内做的按包 cargo 调用——与已修复的 `rust-static` 同属 feature 统一根因，约 180s，另开 issue。
- CI 形状被结构门禁反向锁定：`scripts/verify/checks/task-1.5.mjs` 断言矩阵、步骤顺序、缓存写者集合与 section 映射；`injection-governance.mjs` 对结构门禁本身做注入证明。workflow 与门禁 MUST 在同一次改动内同步，否则治理门禁先红。

## Goals / Non-Goals

**Goals:**

- 删除确证重复的执行：同一检查不在两个 job 跑，同一测试套件不在同一 job 跑两遍。
- 减少实例占用：`perf` 从独立 job 归并，前端从 3 腿收敛为 1 腿，实例数从 20 降到 16。
- 让结构门禁**反向锁定**这些删除：把重复步骤加回来时门禁必须失败，而不是让删除只靠 review 记住。
- 不降低任何检查的断言强度，不缩小平台或包覆盖。

**Non-Goals:**

- 不追求本轮达标 10 分钟（关键路径不在删除对象里，见 Context）。
- 不改 `task-9.1` / `task-13.8` 的 cargo 形态（下一轮）。
- 不删除 Rust Linux 的 `Test` 步骤（见 D1）。

## Decisions

### D1. 放弃「workspace 测试只跑一次」——删了更差

原方案（tier c）是删掉 Rust Linux 的 `Test` 步骤，只让 `rust-static` 的测试清单构建负责编译。实测账：

```
1085s（Rust Linux 冷态）
− 334s（上一轮 rust-static 已省）
− 359s（Test 步骤的执行时间）
+ ≈300s（rust-static 的 --no-run 现编同一批二进制）
≈ 692s，净省约 60s
```

`cargo-test-lists.mjs` 用的正是 `cargo test --workspace --all-targets --all-features --no-run`：删掉 `Test` 不会消除编译，只是把编译挪到 `--no-run` 里现做。代价却是拆掉缓存形态不变量的锚点（`assertNoShapeDrift` 以 `Test` 的 `run` 为基准）、改写 spec 第 374/378 行与 4 条注入证明。

裁决：**不做**。60s 换一个不变量锚点与四条证明的重写，成本大于收益，且现编路径更长、更容易超时。

### D2. `perf` 归并进 `task-gates` core，不归并进 Rust job

- `perf` 只跑 3 个委托场景（`SFI-R05-S03` / `LE-R21-S02` / `PM-R13-S05`），共享同一条 `cargo test -p echo-desktop --all-features --release --test local_edit_performance -- --ignored --nocapture`，独立占一个实例。
- 归并目标是 `task-gates` 的 `core` 分片：它已有 Rust 工具链、`linux-workspace` 缓存（`save-if: false`）与 `pnpm install`，并且**不保存缓存**。`--release` 产物因此不会写进任何 key。
- 不归并进 `rust` job：那是 `linux-workspace` 的写者。在保存缓存前跑 `--release` 会让缓存内容与读取者的 build 形态不符，正好违反上一轮定下的缓存形态不变量。
- 步骤位置放在 `Upload 50k benchmark result` 之后：benchmark 产物是 task-12.5 要求保留的 CI artifact，若 perf 失败且排在它之前，artifact 会因后续步骤跳过而丢失。
- `check-scenario-evidence.mjs` 用 `\bpnpm\s+verify:perf\b` 的**文本存在性**证明 perf gate 接了电；归并后该文本仍在 `ci.yml` 里，语义不变（gate 仍真实执行），因此 `DELEGATED_GATES` 与 `FLOORS` 都不用动。
- 合并后 `task-gates core` 冷态约 363s + 370s ≈ 733s，仍远低于关键路径。

### D3. `task-13.2` 的 `player_smoke` 是更弱的一份，删它反而更诚实

同一 `macos-native-gate` job 里：

| gate | 环境 | skip 行为 | 断言 |
|---|---|---|---|
| `task-8.12` | `DYLD_LIBRARY_PATH=target/Frameworks` + `-- --nocapture` | 命中 `skipping real playback smoke` 即 **FAIL** | `guaranteed_formats_each_load_via_real_libmpv`、`consecutive_loads_then_clean_exit` |
| `task-13.2` | 裸跑 | 拿不到 rpath，**静默 skip** | 只看进程退出码 |

13.2 的报告还宣称 `player_smoke (real libmpv)`——裸跑根本无法支撑这句话。删掉 13.2 的那一行后，该 suite 在 CI 中只由 8.12 以更严格的条件运行；13.2 的报告文案同步改为指向 8.12。

### D4. 前端矩阵 3 → 1，但保留矩阵形态与平台语义名

- 57 个前端测试文件中 0 个读取 `process.platform` / `win32` / `darwin`；`format:check` / `lint` / `typecheck` / `vitest` 全部跑在 jsdom + Node 工具链上，结果与宿主平台无关。
- macOS / Windows 的前端**构建**覆盖不丢：`rust` 的 macOS/Windows `core` 分片与 `rust-desktop` 两个分片都执行 `pnpm --dir apps/desktop build`（Tauri 上下文构建）。
- 实现上保留 `strategy.matrix.include` 的单条目（`{ platform: Linux, os: ubuntu-latest }`）与 `name: Frontend (${{ matrix.platform }})`：检查名仍是 `Frontend (Linux)`，`assertPlatformMatrix` / `assertPlatformName` 的断言形状不变，只把期望集合从三平台改为一平台。
- 不改成无矩阵的 `name: Frontend`：那会让检查名丢失平台语义，违反「CI status checks use stable platform names」的立意，也会让平台映射断言失去对象。

### D5. `injection-proofs` 的前端构建只在 shard 1、2 执行

- shard 1 需要 `dist`：embedded-frontend 证明比对 `dist` 与 shell 二进制。
- shard 2 需要 `dist`：`task-9.1` 的 `cargo check -p echo-app` 在缺 `dist` 时 `generate_context!` 直接 FAIL。
- shard 3 不需要：实测无 `dist` 时 19.1s 通过。
- 原注释「no other shard needs either」写的是 shell 二进制与 `dist` 都不需要，与事实不符（shard 2 需要 `dist`）；一并改正为准确表述。

### D6. `rust-desktop` 删除重复的 `Format check`

`rust` job 的 macOS / Windows `core` 分片已在同一平台、同一 checkout 上跑 `cargo fmt --all -- --check`。`rust-desktop` 只在 `main` 上运行，删掉它的 fmt 后，PR 上的三平台 fmt 覆盖不变（仍由 `rust` 承担），`main` 上也不缺（`rust` 同样在 `main` 运行）。

## Risks / Trade-offs

- **perf 归并后 task-gates core 变长** → 733s，仍低于关键路径 1426s；若未来 core 超过 900s，回退方案是把 perf 拆回独立 job（只是恢复原状，不涉及覆盖变化）。
- **前端单腿后 macOS/Windows 前端构建只在 Rust job 内覆盖** → Rust job 的构建使用同一 `pnpm --dir apps/desktop build`；若将来前端出现平台相关构建分支，需在 `task-1.5` 增加构建覆盖断言，而不是恢复三腿矩阵。
- **结构门禁的反向锁定可能误伤合法新增** → 重复执行断言按「同一 `run` 文本 + 同一 job 外已存在」判定，合法的平台差异执行（不同 `os`）不算重复；新增步骤若被误判，应在门禁里显式登记例外并说明覆盖差异。
- **删除 shard 3 的前端构建依赖「shard 3 无 dist 也能过」的实测** → 该结论来自一次冷 run 的 19.1s 通过记录；若未来 shard 3 新增需要 `dist` 的证明，门禁会因缺少 `dist` 而红，属于显式失败而非静默退化。

## Migration Plan

1. 同一改动内修改 `ci.yml` 与 `task-1.5.mjs`、`task-13.2.mjs`、注入证明与分片计划；先改 workflow 再改门禁，最后跑注入证明自证。
2. 本地验证：`task-1.5.mjs`、`task-13.2.mjs`（在 macOS 上需 staged dylibs）、`check-scale.mjs`、`check-scenario-evidence.mjs`、`validate-scenario-commands.mjs`、三片 `injection-suite.mjs`，最后**独占**运行 `pnpm verify:governance`。
3. 回滚：还原 workflow 与门禁脚本即可；无产品代码、无数据与发布产物变化。

## Open Questions

无。
