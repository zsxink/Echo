# Design

## Context

动机见 `proposal.md — Why`。以下现状事实决定了实现方式（全部在 HEAD `3dbd358` 上实测）：

- **`ci-governance.mjs` 是串行 15 段**，其中 5 段本身就是 cargo 调用；其余 10 段是秒级
  静态门。真正的大头在 **82 个 task 检查脚本内部**：`cargo fmt` 28 处、`cargo clippy`
  25 处、`cargo test` 43 处。单条 `task-2.1.mjs` 就跑整包 `cargo test -p echo-core`
  （无过滤），`task-2.2`~`2.8` 完全重复这一条。
- **`task-3`~`task-7` 是「一测试名一命令」的模板脚本**：`task-N.mjs <task-id>
  <test-name>` 每次启动一次 cargo，并在末尾附带 `clippy` + `fmt`。这 5 个脚本承载
  manifest 中 **233 / 342** 条命令。
- **场景层的「逐场景精确命令」已名不副实**：`run-scenario.mjs` 的 `groupedTestCommand()`
  把 83 条不同的 vitest 命令折叠成一次全量套件；`ECHO_GOVERNANCE_GATE_IN_PARALLEL_JOB=1`
  时把全部 Rust 场景命令直接标为「已被并行治理 job 覆盖」。
- **场景命令的分类**（去重 338 条）：cargo core 132、vitest 83、attestation 59、
  cargo desktop 41、node 检查 19、cargo app 2、其余 2。**按包聚合后只剩 85 条**，
  其中 59 条是逐 ID 的 attestation（不可聚合，各查各的日志）。
- **门禁的现状**：`check-scenario-churn.mjs` 冻结 `maxDuplication 1.8` /
  `maxScenariosPerCommand 16`；`check-scenario-command-proof.mjs` 从**场景命令**里
  解析 `node scripts/verify/checks/<…>.mjs`；`check-verification-validity.mjs` 要求每个
  派生子进程的检查都比较退出码。
- **注入证明当前 35 条**（`node scripts/verify/injection-suite.mjs --list | wc -l`），
  其中 `delegation/cargo-exit-status` 与
  `delegation/runner-propagates-check-failure` 用 `failingBinary("cargo")` 把
  `task-2.1` / `run-task 2.1` 打红，断言 `FAIL 2.1: cargo test`。
- **CI 形状被脚本反向锁定**：`task-1.5` 断言 `ci.yml` 含 9 条精确命令字符串；
  `task-crp` 断言 `release.yml` 的结构；`release-gate` 靠
  `ECHO_GOVERNANCE_GATE_IN_PARALLEL_JOB=1`（`ci.yml:518`）免跑 Rust 场景。

## Goals / Non-Goals

**Goals:**

- 把 fmt / clippy / 整包测试从「每个检查脚本各跑一遍」收敛为「每门一次」，
  且**不减少任何断言**——只减少重复的编译与静态检查。
- 让 `verify:task --all` 的命令数从 342 降到 157，其中最重的 4 个 task 族
  （4.x–7.x）从 218 条降到约 40 条。
- 让场景层从「338 条逐场景命令」变为「约 85 条按域命令 + 523 条显式证据声明」，
  并把「场景被验证过」的判据从**命令里带了过滤器**改成**证据目标存在且被执行**。
- 删除 `run-scenario.mjs` 的整门特判，且不使 CI 墙钟上涨。

**Non-Goals:**

- 不重构 CI job 结构（不并行化 governance 各段、不拆注入 shard、不合并平台 Gate）。
- 不动 `manifest.json` 的 1 空格缩进文本格式（继续文本编辑，禁止 `JSON.stringify` 重排）。
- 不改任何 Rust 生产代码、不改 `cargo` 的编译参数（`-D warnings` 等一律保留）。
- 不聚合 59 条 attestation 场景：每条查的是**自己 ID 的日志**，聚合即失去意义。

## Decisions

### D1 — quality-core 段：fmt / clippy / workspace test 每门各跑一次

`ci-governance.mjs` 收敛为五段，第一段固定为：

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

前两条与 Rust CI 矩阵逐字相同；测试一条是 CI 的严格超集（CI 跑
`cargo test --workspace --all-features`，这里加 `--all-targets`，覆盖集成测试与
bench 目标）。`task-1.5` 断言的是 `ci.yml` 里的字符串，不是本段命令，因此它的
`REQUIRED` 列表不需要改。各 task 检查脚本删除自带的 `fmt` / `clippy` /
无过滤整包 `cargo test`，只保留**定向断言**（具体测试过滤器 + 源码结构判断）；
`task-1.5` 的本地 `RUST_STEPS` / `FRONTEND_STEPS` 执行也一并删除，只留 ci.yml
结构断言（这些前端步骤由 `task-1.2` 与 quality-core 之外的独立门承担）。

- 备选：让检查脚本继续自带 fmt/clippy，只把 governance 改成并行。否——CI 并行
  只是把重复摊到多个 job，单机 `pnpm verify:governance` 仍是 82 次重复启动；
  而 issue 的目标是降低**维护面与单机耗时**。
- 备选：用 `cargo nextest` 合并启动。否——引入新工具链会连带改 `check-toolchain`、
  Rust 矩阵与开发者本地环境，超出本阶段范围。

**必须保留的子进程调用**：`task-2.1` 仍需跑一条**带过滤器**的 cargo 测试
（`cargo test -p echo-core --all-features <ids-filter>`），否则
`delegation/cargo-exit-status` 与 `delegation/runner-propagates-check-failure` 两条注入
证明失去注入面。这两条证明的 `expect` 正则需同步更新为新过滤器文本。

### D2 — `task-3`~`task-7` 改为「一 task 一条命令、按模块分组启动」

签名从 `<task-id> <rust-test-name>` 改为：

```
node scripts/verify/checks/task-N.mjs <task-id> <test-name>...
```

运行器收集该 task 的全部测试名，按每名的**前两段模块路径**分组（例如
`application::import::logic::tests::x` → `application::import`；不足两段的裸测试名
自成一组），每组执行一次 `cargo test -p <pkg> --all-features <该组前缀>`，然后
**逐条断言每个声明的测试名出现在 `test <name> ... ok` 行中**。任一测试名未出现 →
失败并列出缺失名；零测试执行 → 失败（沿用现有「过滤器零命中即失败」语义）。

实测分组数：45 个 task 共 **73 组**（原来 230 次启动）；其中 27 个 task 只需 1 组。
`manifest.json` 中 `3.x`~`7.x` 的 230 条命令合并为 45 条（一 task 一条）。

- 备选：保留逐测试命令但去掉其中的 clippy/fmt。否——那样仍有 230 次 cargo 启动，
  收益只有静态检查那一半。
- 备选：用单条 `cargo test -p <pkg>`（无过滤）覆盖整个 task。否——那是每 task 跑
  整包套件，45 次全量比 230 次定向慢得多，而且会把无关模块的失败算到本 task 头上。
- 备选：断言「过滤器命中数 > 0」而不逐名断言。否——那会把「某个测试被删掉」变成
  静默通过，正是 `engineering-governance`「门禁不得静默通过」禁止的形态。

### D3 — 场景命令按域聚合，每个场景显式声明证据

`scenario-commands.mjs` 从 `COMMANDS: id → command` 改为 `SCENARIOS: id → {command, evidence}`：

- `command` 是**按域聚合后的**命令（如 `cargo test -p echo-core --all-features`）；
- `evidence` 是**非空数组**，列出该场景真正依赖的全部验收目标——一个场景可能同时依赖
  多个测试（例如 `SFI-R12-S01` 的导入、watcher 抢占、逐资源失败、提交边界、离线重建）。
  元素形如 `{kind: "rust", target: "<test-name-or-filter>"}`、
  `{kind: "vitest", target: "src/x.test.tsx", name?: "<test name>"}`、
  `{kind: "node", target: "scripts/verify/checks/task-9.1.mjs"}`、
  `{kind: "attestation", target: "<scenario id>"}`。
- 单目标场景也必须写成单元素数组，避免「对象 / 数组」两种形状在门禁里分叉。

`manifest.json.scenarios[]` 条目变为 `{id, title, command, evidence}`。

聚合后命令数约 85 条：core 1、desktop 1、app 1、player_smoke 1、perf 1、
vitest 1、node 检查 19、attestation 59、governance 相关 0（见 D4）。

- 备选：只聚合、不声明证据。否——聚合后「命令成功」不再蕴含「这个场景的测试跑过」，
  等于把 523 条验收降级成 85 条；那是**降低**严格度。
- 备选：证据复用 `docs/traceability.md` 的「测试」列。否——该列是给人读的中文描述，
  不是可解析目标，且它本身是 P2-5 要收敛的手工表。

### D4 — 两个静态门随之改造（churn → 证据覆盖；proof → 读证据）

**`check-scenario-churn.mjs` 退役，由 `check-scenario-evidence.mjs` 接替。**
理由：聚合必然把重复度从 1.55x 推到约 6x，与冻结的 1.8x 天花板直接冲突；而 churn 门
原本守的是「一条命令坏掉会同时抹掉一簇场景」——聚合后这个风险**本来就不存在了**
（同一域本来就共享一条命令）。真正的风险转移为「场景指向了错误的证据」，
新门守这一条：

- 每个场景必须有非空 `evidence` 数组，且每条元素有合法的 `kind` 与 `target`；
- `rust` 证据必须能在对应包的 `cargo test -- --list` 目标集合中命中；
- `vitest` 证据文件必须存在，带 `name` 时该名称必须出现在文件内容中；
- `node` / `attestation` 证据目标必须存在（attestation 还要求 target 等于该场景 ID）；
- 冻结一个「场景数 × 证据数」下限（当前 523 个场景，证据条数按迁移时的实测值冻结），
  只增不减；删掉一个场景的证据元素必须同时说明该场景验收为何不再需要它。

**`check-scenario-command-proof.mjs` 改读 `evidence.node.target`**：它现在的输入是
场景命令里的 `node scripts/verify/checks/*.mjs`，聚合后命令里不再出现这些路径。
分类逻辑（proven / self-contained / pure delegation / declared）与
`scenario-command-proof-declarations.json` 一字不改，只换解析来源。

- 备选：保留 churn 门并把天花板调高到 6x。否——那等于把「重复度」这个指标彻底
  失去意义后还挂着它，读者会误以为聚合是违规。

### D5 — 整门特判改为显式 delegation，并同步 release-gate

三条场景按 THEN 的字面要求分两类处理：

| 场景 | THEN 要求 | 处置 |
|---|---|---|
| `SFI-R12-S01` 导入契约回归 | 导入/watcher 抢占/逐资源失败/提交边界/离线重建等具体测试 + 跨平台证据 | 改指具体子门（core 域聚合命令 + 定向检查） |
| `LD-R10-S01` 可执行验证 | 字面包含 `pnpm verify:governance` 通过 | 保留整门口径，登记为 `delegatedTo: "governance"` |
| `PHA-R01-S02` 回归质量门 | Rust/前端/格式/静态检查 + 登记场景全通过 | 同上 |

删除 `run-scenario.mjs` 的 `ECHO_GOVERNANCE_GATE_IN_PARALLEL_JOB` 分支，代之以：

- 注册表条目可声明 `delegatedTo: <gate>`（只允许 `"governance"`，静态门校验）；
- `run-scenario.mjs` 新增 `--delegated=<gate>` 参数：只跳过**显式声明**该 gate 的场景；
- 删掉「所有 `cargo test` 命令一律视为已覆盖」这条按命令模式推断的分支——聚合后
  cargo 场景只剩 5 条命令，让它们在 release-gate 真实执行更忠实，且特判省下的
  「~173 次启动」已不存在。

同步修改：

- `.github/workflows/ci.yml` 的 `release-gate`：env 换成
  `node scripts/verify/checks/task-13.9.mjs --delegated=governance`；
- `checks/task-13.9.mjs`：把 env 透传改为参数透传；
- `scripts/verify/self-test.mjs` 的 `reusedRustScenarios` 用例：改断「显式声明的
  场景被跳过、未声明的 cargo 场景仍执行」；
- `check-verification-validity`（或新静态门）断言 `delegatedTo` 只能指向真实存在于
  `ci-governance.mjs` 的段，防止声明一个根本不跑的 gate。

- 备选：把 `LD-R10-S01` / `PHA-R01-S02` 也改指子门。否——它们的 THEN 明确要求整门
  通过，改指子门是把验收降级成「前端那一半」，属于降低严格度。
- 备选：直接删特判、三条都保留整门命令。否——release-gate 会在场景执行中再次跑
  `ci-governance`（含覆盖率和注入证明），既有递归风险又让 CI 墙钟爆炸。

### D6 — P2-6 的过期数字：改为「现算」而不是「改数」

脚本头注释里的计数（`spec-scenarios.mjs` 的 "160 IDs"、`task-13.9.mjs` 的 "45 native
manifests"）一律改为**不写死数字**的表述，或改写成脚本自己运行时输出的值；需要保留
具体数字时，必须在同一 change 内由脚本输出核对。当前真实值：523 场景、
59 条 attestation 场景、36 条注入证明、140 个 task。

- 备选：只把数字改成新值。否——它会在下一次 spec 变更后再次腐烂，这正是
  `docs/arch-standards-remaining.md` 已经踩过的坑。

### D7 — 执行顺序与验证点

依赖顺序（tasks 按此拆）：

1. P2-6（纯注释，零风险）
2. P2-1 quality-core 段 + 各脚本去重（含两条 delegation 证明的 expect 更新）
3. P2-2 task 运行器签名 + manifest 合并
4. P2-3 聚合 + 证据声明 + 两个静态门改造
5. P2-4 三场景改指向 + 删特判 + CI/self-test 同步

每步结束都必须 `pnpm verify:governance` 退出 0 且 `git status --porcelain` 为空。
第 4 步是唯一允许 manifest 与场景层同时变动的步骤，必须一次跑通
`gen-scenario-manifests.mjs --write` + 三个静态门 + `run-scenario --automated`。

## Risks / Trade-offs

- **[聚合后失败归因变粗]** 一条 `cargo test -p echo-core --all-features` 失败时，
  `run-scenario` 只能报「该域命令失败」，523 条场景里哪条被牵连说不清。
  → 缓解：失败时解析输出里的 `FAILED <test-name>`，把命中的证据目标映射回场景 ID 并列出。

- **[证据声明可能写错]** 声明一个存在但**与本场景无关**的测试，会重新引入假绿。
  → 缓解：`check-scenario-evidence` 只证明「目标存在」，不证明「语义正确」；语义正确由
  `check-scenario-command-proof` 的注入证明与 review 承担。tasks 中要求逐域复核，
  不允许批量替换（沿用 issue P2-3 的警告）。

- **[删除 churn 门被看作降低严格度]** → 缓解：新门的断言强度必须不低于 churn 门
  （存在性 + 运行时观测 + 冻结下限），并为它补至少一条注入证明；替换掉 churn 的
  注入条目后再新增 1 条，注入证明总数 MUST 不低于 36（基线 35）。

- **[去掉脚本内 clippy 后，单独跑 `pnpm verify:task -- 2.1` 不再做静态检查]**
  → 缓解：静态检查由 Rust CI 矩阵（三平台）与 quality-core 段承担，且这是
  **有意**的：单任务门只做定向断言。`task-1.5` 仍断言 ci.yml 里有这三条命令。

- **[manifest.json 大规模文本编辑出错]** → 缓解：沿用既有约定（1 空格缩进、`],` 前留空行、
  禁止 `JSON.stringify`），用 `gen-scenario-manifests.mjs --write` 生成 scenarios 段；
  tasks 段按 task id 逐块文本替换，每块后跑 `node -e` 校验条数。

- **[Windows/macOS 上聚合命令的耗时分布变化]** → 缓解：在 tasks 中记录三平台矩阵的
  CI 结果对比（至少 Linux + Windows），若某平台显著变慢则回退该域的聚合粒度。

## Migration Plan

1. 分支 `codex/issue-110-stage3-de-duplicate-quality-gates`，基于合并后的 `main`。
2. 按 D7 的 5 步逐条提交，每步独立可回滚（`git revert` 单步即可回到前一绿点）。
3. 第 4 步同时改动 `scenario-commands.mjs` + `manifest.json` + 两个静态门，
   **必须一次原子提交**，否则中间态无法通过任何门。
4. CI 侧只有第 5 步触碰 `ci.yml`；回滚该步即恢复特判，无需回滚前四步。
5. 全部完成 → 同步 delta spec 到主规格 → 归档 change → 提 PR。

## Open Questions

（无。D3 的聚合粒度与 D4 的门禁替代方案已在 propose 阶段与维护者确认。）
