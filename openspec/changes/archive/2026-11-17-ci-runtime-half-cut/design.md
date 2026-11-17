# Design

## Context

`feat/ci-runtime-under-10-min` 已把 PR 实例数从 18 压到 15，但最近一次 run（`44037252640`）仍是 7233 runner-秒、17m13s 墙钟。Release gate 的 1030s 中约 433s 是 `cargo test --workspace` 与完整 Vitest 的**第二次逐字节执行**；task-gates 的 1360s 中大部分是 workspace/frontend 套件的子集；injection-proofs 的 1762s 是治理脚本自身的 mutation 元测试。PR #157 已把「同一检查不得重复」写成 MUST NOT，但当前 CI 仍违反它。

约束：CI 结构由 `task-1.5.mjs` 与 `injection-governance.mjs` 反向锁定；`engineering-governance` spec 要求 PR 三平台 fmt/clippy、task 分片覆盖、场景委托指向真实 gate；任何删除都必须同步门禁与 delta spec。产品正确性检查（workspace test、frontend test、平台 Gate、perf、fault injection、50k 预算、E2E）不得消失。

## Goals / Non-Goals

**Goals:**

- PR 机器时间从 7233s 降到约 3000s（−58%），关键路径从 1030s 降到约 500–600s。
- 删除的每一项都是「已被其他 job 覆盖」或「只验证治理脚本自身」的执行，不是产品检查。
- 结构门禁继续反向锁定新形态：重新加回重复执行、把 main-only 检查改成永久跳过、把场景委托指向不存在的 gate，都必须失败。

**Non-Goals:**

- 不优化单条 cargo 命令的编译时间、不引入新的缓存后端、不改 release.yml。
- 不删除任何产品测试或平台 Gate。
- 不把 injection proof 套件搬进新的定时 workflow；它保留为本地可执行套件，CI 只保留静态登记检查（见 Decisions）。

## Decisions

### D1. 删除 injection-proofs job，proof 套件保留为本地命令

injection proofs 证明的是「治理门禁在违规时确实会失败」，不是产品行为。它每次 PR 消耗 3 个实例、1762s，且其中 shard 3 只有 5 条 proof 却要付完整 checkout/toolchain 成本。删除 job 后，`node scripts/verify/injection-suite.mjs` 仍可在本地全量运行；CI 不再把元测试当作合并门槛。

替代方案：把 3 个 shard 合并成 1 个（约 859s）或只在 main 运行。合并仍保留 1 个实例和完整编译；只在 main 运行会让 main 关键路径增加约 859s，与 <10 分钟目标冲突。选择删除 PR job，保留本地套件。

### D2. task-gates 收敛为 core 单分片，保留 4 条独有门禁 + perf

逐条核对 42 条 task 的命令后：

- `3.1` / `4.1` / `5.1` / `6.1` 是 `cargo test -p echo-core` 的模块子集；`7.1` / `8.1–8.11` / `10.6` / `11.1` / `11.2` / `11.4` / `16.2` 是 `cargo test -p echo-desktop` 的子集；`10.1–10.5` / `10.7–10.10` / `11.3` / `11.6` / `12.1–12.3` 是 Vitest/typecheck/lint 的子集。它们全部由 Linux workspace test 或 Frontend job 覆盖。
- `1.1`（workspace 清单）、`1.3`（锁文件 + self-test）、`12.5`（50k 性能预算与 artifact）、`13.3`（故障注入报告）没有等价覆盖，保留。
- `13.1`（headless Chromium E2E）没有等价覆盖，迁入 Frontend job，随前端矩阵一起运行。
- `pnpm verify:perf` 必须保留在 PR 与 main，继续放在 core 分片；该分片缓存 `save-if: false`，`--release` 产物不会污染 `linux-workspace`。

被删除的 task 在 `manifest.json` 中标记 `archived: true`，归档理由写明承载 gate（workspace test / frontend test）。`run-task --all` 会跳过它们，单独点名仍可运行。

### D3. macOS/Windows core 分片合并进 main-only 的 rust-desktop

PR 上 `rust` 只保留 Linux `workspace` 分片；`rust-desktop` 的 macOS/Windows 分片改为 `--workspace` 包集合并增加 `Format check` 与 `Clippy`，只在 main 运行。这样：

- PR 仍有 Linux 全量 fmt/clippy/test，且 macOS/Windows platform Gate 会编译并测试平台代码；
- main 仍有 macOS/Windows 的全量 fmt/clippy/test，只是从两个 job 合并成一个；
- 缓存 key 从 6 个减到 4 个（`linux-workspace`、`macos-desktop`、`windows-desktop`、`governance-static`），`macos-core`/`windows-core` 不再写入。

替代方案：保留 core 分片但用 step-level `if` 跳过。job 仍会启动并支付 checkout/toolchain 成本，省不下实例；且 job-level `if` 不能引用 `matrix`。合并进 `rust-desktop` 是唯一不新增 job 的干净做法。

### D4. Release gate 通过显式 delegatedTo 委托 workspace/frontend 套件

在 `scenario-commands.mjs` 的注册表层面给命令为 `CORE_ALL` / `DESK_ALL` / `APP_ALL` 的条目补 `delegatedTo: "workspace-test"`，给 `VITEST_ALL` 的条目补 `delegatedTo: "frontend-test"`。这不是 runner 的命令形态推断：委托关系写在注册表里，生成到 manifest，`check-scenario-evidence.mjs` 校验 gate 在 `ci.yml` 中真实存在。

`run-scenario.mjs` 的 `--delegated=` 逻辑不变：只有显式声明了该 gate 的场景才会被标记为 covered。Release gate 的命令改为 `--delegated=governance,perf,workspace-test,frontend-test`。Frontend job 的 `Test` 步骤增加 `--reporter=verbose`，使其输出与场景证据声明一致。

替代方案：在 runner 里按命令形态自动跳过。这正是 PR #157 注释里警告过的「聚合后启发式跳过真实工作」，不采用。

### D5. coverage 移到 main，governance-static 拆分步骤

`Run governance static gates` 在 PR 与 main 都执行 `registry,static`；新增 `Run coverage gate` 步骤，`if: github.event_name != 'pull_request'`，执行 `--sections=coverage`。`Install coverage tooling` 同样 main-only，避免 PR 安装 `cargo-llvm-cov`。`task-1.5` 断言 coverage 步骤的 main-only 条件与命令，保证它不会从两边同时消失。

### D6. 16.1 静态契约保留，9.7/9.8 的 Linux 空转删除

`task-16.1.mjs` 增加 `--static-only`：跳过 `cargo check` 与 `cargo test`，只保留「单一 owner、禁止 `url.to_string()`、path-typed 契约」的静态断言。manifest 中 `16.1` 的命令改为 `node scripts/verify/checks/task-16.1.mjs --static-only`，在 governance-static 中运行。`open_targets` 行为测试由 workspace test 覆盖。Linux 的 `9.7 9.8` 在无 vendored libmpv 时是空转，真实执行在 Windows platform Gate（9.7/9.8）与 macOS platform Gate（9.7），删除 Linux 步骤。

### D7. embedded-frontend 检查迁到 macOS platform Gate

`check-embedded-frontend.mjs` 只读取已构建的 `dist/index.html` 与一个已构建的 `Echo` 二进制，不调用 cargo。macOS platform Gate 的 `tauri build` 会产出 `target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents/MacOS/Echo`，因此在该 job 增加一步：

```
node scripts/verify/checks/check-embedded-frontend.mjs \
  --bin target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents/MacOS/Echo
```

这保留 `embedded` governance section 的执行位置，且不新增编译。

### D8. 纯文档 paths-ignore 采用审计白名单

只忽略已确认不是门禁输入的路径：`docs/arch-standards-remaining.md`、`docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md`、`docs/interface-terminology.md`。`task-1.5` 把该清单固定为常量并断言 `ci.yml` 的 `paths-ignore` 与之一致；任何新增条目必须先确认不是门禁输入。

审计时排除了三个看似文档、实际是门禁输入的目录/文件：`README.md`（task-17.1/18.1 读取许可与 Windows 基线）、`openspec/changes/archive/**`（audio-effects 证据门禁读取归档证据）、`artifacts/**`（`check-native-attestation.mjs` 读取 attestation 日志）。因此可跳过的范围比初稿小，但不会让任何门禁输入绕过 CI。

## Risks / Trade-offs

- [删除 injection-proofs 后，治理门禁的失败证明不再每次 PR 执行] → proof 套件与 shard plan 保留在仓库，PR 模板/本地验证命令继续要求运行；`check-scenario-command-proof.mjs` 仍静态要求 scenario-backed 检查有 proof 登记。代价是 proof 腐化只能在本地或人工运行时发现。
- [PR 不再跑 macOS/Windows fmt/clippy] → 平台 Gate 仍编译并测试平台代码；main 的 `rust-desktop` 仍跑全量 fmt/clippy/test。代价是 PR 上可能合入一个只在 macOS/Windows `clippy` 才报的 lint；由 main 的必跑检查兜底。
- [归档 37 条 task 后，个别子集断言（例如某个模块测试的特定过滤器）不再被单独点名] → 这些断言对应的测试仍在 workspace test 中执行；manifest 归档理由记录承载 gate，`task-1.5` 校验每条理由指向的 gate 在 `ci.yml` 中真实存在。代价是失败信息不再以 task id 定位，需要看 workspace test 的测试名。
- [Release gate 委托后，场景证据不再在 release gate 输出中逐条观测] → 委托 gate 执行完整套件，任一测试失败都会让该 gate 红；`check-scenario-evidence.mjs` 静态校验委托 gate 存在。代价是 release gate 日志不再包含每条场景的 `ok: scenario` 证据行，证据观测转移到被委托 job 的完整测试输出。
- [paths-ignore 白名单可能漏掉新的门禁输入] → 白名单由 `task-1.5` 固定；新增门禁输入时必须同步修改常量与断言，review 可见。

## Migration Plan

1. 先改注册表与 manifest（`delegatedTo`、`archived`、`16.1` 命令），运行 `gen-scenario-manifests --write`。
2. 改 `ci.yml` 结构，再改 `task-1.5` / `ci-governance` / `check-scenario-evidence` / injection proofs。
3. 本地按顺序验证：`task-1.5` → `check-scenario-evidence` → `injection-suite`（更新后的 proof）→ `pnpm verify:governance`（独占）→ `task-13.9 --skip-static --delegated=...`。
4. 推送后观测一次 PR run：机器时间目标 ≤3600s，关键路径 ≤600s；若 release gate 仍 >600s，再评估把剩余 cargo 子集命令委托给 workspace-test（不改变本次 spec）。

回滚：本次改动只涉及 CI 与治理脚本，回滚 commit 即可恢复旧 job 与旧门禁；无数据迁移。
