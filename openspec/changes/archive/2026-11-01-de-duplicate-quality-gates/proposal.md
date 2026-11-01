## Why

Issue #110 阶段 3（P2-1/P2-2/P2-3/P2-4/P2-6）：`pnpm verify:governance` 的耗时几乎全部来自
cargo 的重复启动，而不是来自真正不同的断言。HEAD（`3dbd358`）实测：

| 维度 | 现状 |
|---|---|
| `verify:task` | 140 个 task / 342 条命令（342 条全部去重，即无重复命令，但**每条都是一次 cargo 启动**） |
| 场景命令 | 523 场景 / 338 条去重命令（1.55x，最差簇 16） |
| 检查脚本里的重复调用 | `cargo fmt` **28** 处、`cargo clippy` **25** 处、`cargo test` **43** 处 |
| 无过滤全跑 `cargo test -p echo-core` | **9** 处（`task-1.8`、`task-2.1`~`2.8`） |
| 最重的 task 族 | `task-5` 81 条、`task-4` 56 条、`task-7` 49 条、`task-6` 32 条（合计 218 条） |

这些重复调用并不改变任何验收语义：`task-2.1` 跑的是整包 `cargo test -p echo-core`，
`task-2.2` 跑的还是同一条；`task-5.mjs` 的 81 条命令每条都先跑一遍 `clippy` + `fmt`。
它们只是把同一次编译与同一次静态检查摊进了 82 个脚本。

更要紧的是**场景层的假绿风险**：场景命令是逐场景的 cargo/vitest 过滤器，过滤器零命中
本应失败（`engineering-governance` 已要求），但 `run-scenario.mjs` 已经在运行时把前端
命令折叠成一次全量 vitest、并在 CI 并行 job 里把 Rust 命令标记为「已被覆盖」——也就是说
「逐场景精确命令」在实践中早已退化为「按域批量」，只是登记表还维持着逐场景的假象。

本 change 把这件事做正：**命令按域聚合、每个场景显式声明自己的验收证据**，让「场景确实
被验证」从「命令恰好带了过滤器」变成「证据目标存在且被执行」。

## What Changes

- **P2-1 消除重复的 fmt/clippy/test 调用**：`ci-governance.mjs` 收敛为 5 段（quality-core /
  static gates / coverage / embedded-frontend / injection），其中 quality-core 只跑一次
  `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features
  -- -D warnings`、`cargo test --workspace --all-targets --all-features`。28 + 25 + 43 处
  检查脚本内自带的 fmt/clippy/整包测试改为只保留定向断言。
- **P2-2 收敛 `verify:task` 的「一测试名一命令」**：`task-3`~`task-7` 改为
  `task-N.mjs <task-id> <test-name>...`，由运行器把测试名按「前两段模块路径」分组，
  每组只启动一次 cargo（实测 45 个 task 共 73 次启动，取代原来的 230 次），并逐条断言
  每个测试名都出现在输出中（任一名缺失或零命中仍然失败）。`manifest.json` 中这 45 个
  task 的 230 条命令合并为 45 条，全局 342 条降到 157 条。
- **P2-3 场景命令按域聚合 + 证据声明**：`scenario-commands.mjs` 的 338 条去重命令聚合为
  按域命令（预计约 85 条），每个场景改为携带 `evidence` 数组（它真正依赖的一条或多条
  测试名/测试文件/检查脚本）。
  `validate-scenario-commands.mjs` 由「命令里的过滤器命中」改为「证据目标存在且被执行」；
  `check-scenario-churn.mjs` 的重复度棘轮被「场景证据覆盖」门取代（聚合必然抬高重复度，
  与 1.8x 天花板直接冲突，不能两存）。
- **P2-4 拆掉「场景命令指向整门」的反模式**：`SFI-R12-S01` 改指具体子门；`LD-R10-S01` /
  `PHA-R01-S02` 的 THEN 字面要求整门通过，不能改指子门（那是削弱验收），改为**显式
  delegation 声明**——场景在注册表里标明「由 governance job 覆盖」，`run-scenario`
  用显式 CLI 参数只跳过这些声明项，而不是像现在这样按命令字符串与「所有 cargo 测试」
  一刀切。**必须与 `release-gate`（`ci.yml:518`）同 change 改**：该 env 是 release-gate
  免跑 ~173 条 cargo 场景的依据，只删脚本不同步 workflow 会让 release-gate 重跑全部
  Rust 场景。
- **P2-6 修正脚本头过期数字**：`spec-scenarios.mjs:17`（"160 IDs" + 旧分域计数，
  实际 523 条且已含 SYN 域）、`task-13.9.mjs:19`（"45 native manifests"，实际 59 条
  attestation）。`check-scenario-churn.mjs` 在 P2-3 整文件退役，其历史数字随文件消失；
  `scenario-commands.mjs` 头部的 523 由脚本自身输出核对，不再手写分域计数。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `engineering-governance`：修改「门禁不得静默通过」——场景命令可以按域聚合，但每个场景
  MUST 显式声明其验收证据，且证据目标 MUST 存在并被实际执行；仅靠「命令里带了过滤器」
  不再算作已验证。同时新增场景：聚合后证据目标缺失 / 证据未被执行 / 聚合命令掩盖了
  某个场景的证据，都必须失败。

## Impact

**受影响脚本**：`scripts/verify/{ci-governance.mjs, run-task.mjs, run-scenario.mjs,
scenario-commands.mjs, manifest.json, validate-scenario-commands.mjs,
check-scenario-churn.mjs, check-scenario-command-proof.mjs, gen-scenario-manifests.mjs,
spec-scenarios.mjs}`、`scripts/verify/checks/{task-3,task-4,task-5,task-6,task-7}.mjs`
以及 28 + 25 + 43 处内联 fmt/clippy/test 的 task 检查脚本、`checks/task-13.9.mjs`。

**受影响工作流**：`.github/workflows/ci.yml`（`release-gate` 的 env 与三个场景的落点；
`governance` job 的调用参数）。仅做 P2-4 必需改动，不重构 job 结构。

**受影响文档**：`docs/traceability.md`（若有场景命令列变化）、
`docs/native-attestation-playbook.md`（聚合后的场景执行语义与证据声明）、
`docs/arch-standards-remaining.md`。

**不变式（不得在本 change 中放宽）**：

- 523 条场景登记与它们的 ID 集合、`reconcile-scenarios.mjs` 三方对账、`prd-matrix.mjs`
  的 A1–A14 归属全部不变。
- 注入证明条目数不减（`injection-suite.mjs --list` 当前 35 条；本 change 替换 1 条、
  新增 1 条，完成后为 36 条），任何新增/替换的检查必须带「注入违规即失败」的证明。
- `-D warnings`、coverage 阈值 90%、`scale-baseline.json` 只减不增、`build-purity` 门
  一律不放宽。
- 不修改任何 spec 的 WHEN/THEN 语义（`engineering-governance` 的那一条是本 change
  自身引入的契约强化，不是对既有验收的削弱）。

**明确不在范围内**：

1. CI job 结构重构（并行化 governance 各段、拆注入证明 shard、合并/删除平台 Gate、
   前端三平台收敛）——见 issue §8.3，留作后续 change。
2. P2-5 的三份手工同步表收敛（`consolidate-traceability-tables`）。
3. `scripts/audio-effects/` 的 Python 工具链。
4. 任何 Rust 生产代码的修改（本 change 只动验证层）。
