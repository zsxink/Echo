# PR CI 10 分钟设计

- 日期：2026-11-02
- 状态：待 builder check / 待用户批准（未进入 OpenSpec propose，未开始实现）
- 目标 change 名：`ci-pr-under-10-min`（批准后创建 Issue、分支与 OpenSpec change）
- 相关规格：`openspec/specs/engineering-governance/spec.md`
- 相关文档：`docs/arch-standards-remaining.md`、`docs/ci-pipeline-review-2026-10-28.md`

## 1. 背景与目标

PR #123 的 CI（run `41323932057`）从 15:58:55 到 16:20:41，墙钟 **21 分 46 秒**。目标是让 PR 的 CI 墙钟稳定低于 **10 分钟**，手段限定为删减重复、迁移检查、分片并行、修正脚本内的无效开销；不新增付费 runner，不删除验收内容。

用户已明确选择的取舍（本设计的前提，不再重新讨论）：

- PR 上保留三平台 `cargo fmt` / `cargo clippy` 与平台门禁；
- PR 上只在 Linux 跑完整的 `cargo test --workspace`；
- Windows 与 macOS 的完整工作区测试移到 **push 到 main**（以及任何非 `pull_request` 事件）；
- 前端三平台矩阵保持不变（工程治理规格要求）。

### 非目标

- 不改 `release.yml` 的发布流程与三平台产物。
- 不删减测试、场景或验收证据；只允许迁移执行位置、分片、复用已构建产物。
- 不把前端矩阵缩成单平台。
- 不以「加缓存」作为唯一手段；缓存只作为并行之外的辅助。
- 不优化 main 的墙钟；main 允许保持较慢，只要 PR 达标。

### 反目标（即使数字达标也算失败）

- 任何检查在 PR 和 main 都不再执行。
- 为了让某个 job 变绿而放宽断言、删掉证据或把 `--all-features` / `--all-targets` 退回。
- 用注释或字符串骗过 `check-scenario-evidence.mjs` 的「CI 中有该 gate」检查。
- 让 Windows/macOS 的平台专属行为只在本地或只在人工举证中出现。

## 2. 基线测量

墙钟由最慢 job 决定；基线中最慢的 6 个 job 都超过 8 分钟。

| Job | 墙钟 | 关键步骤（秒） |
|---|---:|---|
| Task gates | 1304s | `Verify task gates` 1212s（43 个 task 串行） |
| Governance gates | 1015s | `Run governance gates except mutation proofs` 909s |
| Rust (Windows) | 983s | `Test` 835s；`Clippy` 49s |
| Rust (macOS) | 830s | `Test` 486s；8.12/13.2 222s；`Clippy` 33s |
| Release gate | 827s | `Run release gate` 740s |
| Gate injection proofs | 598s | 注入套件 413s；shell 构建 71s；setup ≈ 100s |
| Rust (Linux) | 420s | `Test` 314s；`Clippy` 21s；setup ≈ 80s |
| Windows platform Gate | 313s | 原生确认测试 + 9.7/9.8/17.1/18.1 |
| macOS platform Gate | 306s | 1.9/1.10/9.7 |
| Frontend (Linux/macOS/Windows) | 70/72/80s | format/lint/typecheck/test/build |
| Dependency audit | 38s | 1.7 |
| Linux platform Gate | 26s | 9.7/9.8 |

基线 PR 总墙钟 21m46s；最近一次 main push（run `41356319878`）为 19m21s。

### 关键步骤成本

- Task gates 内部：11.4 ≈ 195s，1.3 ≈ 171s，7.1 ≈ 108s，16.1 ≈ 99s，6.1 ≈ 53s，1.8 ≈ 32s；约 20 个前端 task 各 33–37s，13.1 ≈ 20s。
- Governance 内部：quality-core tests 266.5s，validate-scenario-commands 323.4s，coverage 212.4s，其余静态检查合计约 60s。
- Release gate 内部：静态预检 282s，场景执行 458s（其中性能场景 220s，其余场景 238s）。
- Rust Windows 的 `Test` 835s 中，编译约 8 分钟、执行约 5 分 44 秒；`echo-core` 测试二进制本身 265.75s。

## 3. 根因

### R1：Task gates 是一个 43 项串行队列

`run-task.mjs` 按给定顺序串行执行。前端相关 task 各自重新跑 `typecheck` 和一次聚焦 Vitest；`1.3` 的 self-test 在最后一项 `--all` 运行时使用了真实 PATH，导致 fixture 里的 `task-4.mjs` 调用了真实 `cargo test -p echo-core`（约 171s 的假开销）；`11.4` 在 `7.1` 之后以**不同 feature 集**再次编译 `echo-core` / `echo-desktop`，重复付出编译成本。

### R2：Governance 与 Rust Linux / Release gate 重复

Governance 的 `quality-core` 段重复了 Rust Linux 已经执行的 fmt/clippy/test；`validate-scenario-commands` 在 Governance 跑 323s，Release gate 的静态预检又跑一遍；`task-13.9` 在 Release gate 里再次执行 reconcile/validate/gen。PR 上同一件事被做了两到三次。

### R3：Release gate 把静态校验和性能测试串在自己身上

Release gate 740s 中，282s 是静态预检，458s 是场景执行；性能场景 220s 与其余场景共享同一个 cargo target 锁，导致场景只能串行等待。性能场景本身不需要 webkit、不需要前端产物，却被放在最重的 job 里。

### R4：注入证明单 job，Windows/macOS 满分测试在 PR 上

注入套件 413s 单 job 执行，且要先构建 shell（71s）。Windows/macOS 的完整测试（835s/486s）在 PR 上执行，但平台专属风险已由 clippy 与平台 Gate 覆盖；完整测试的价值主要在 main 的回归兜底。

### R5：`cargo test -- --list` 没有对齐 `--all-targets`，重复编译 doc-tests

`validate-scenario-commands` 与 `check-scenario-evidence` 都用 `cargo test -p <pkg> --all-features -- --list` 获取测试清单。这个形态会额外驱动 **doc-test 的 rustdoc 构建**，并在 `--workspace` 测试之后再次付出可观的构建时间：基线 Governance 中 `quality-core tests` 已经跑完 `cargo test --workspace --all-targets --all-features`（266.5s，编译 190s + 执行 76s），紧接着的 `validate-scenario-commands` 仍花了 **323.4s**；而它之后的 `check-scenario-evidence` 只用 12.5s。Release gate 的静态预检 282s 也是同一原因。

本机实测（同一 target，热构建）：

```text
cargo test -p echo-core --all-features -- --list              # 22.2s（doc-tests 构建）
cargo test --workspace --all-targets --all-features --no-run  # 21.7s（补 echo-desktop/echo-app）
cargo test -p echo-core --all-features -- --list              # 1.1s（doc-tests 已缓存）
cargo test -p echo-core --all-targets --all-features -- --list # 0.9s（跳过 doc-tests）
```

因此把清单调用改成 `--all-targets --all-features`、与完整测试步骤同一构建形态，`rust-static` 才能真正在 Rust (Linux) 的热构建上只花几秒；否则 D3 的收益不成立。

## 4. 决策

### D1：PR 与 main 分层覆盖（已获用户批准）

- 三平台 `rust` job 都保留 `cargo fmt`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- `Test` 步骤条件改为：

  ```yaml
  if: github.event_name != 'pull_request' || matrix.platform == 'Linux'
  run: cargo test --workspace --all-targets --all-features
  ```

  即：PR 只有 Linux 跑完整工作区测试；push 到 main、merge queue、手动触发等非 PR 事件三平台都跑。命令从 `--all-features` 改为 `--all-targets --all-features`，与原先 Governance 的 `quality-core tests` 覆盖并集一致；仓库没有 doctest，`--all-targets` 不丢 doc 测试。

- macOS 的 8.12/13.2（libmpv smoke + native e2e）仍在 macOS rust job 的 PR 上执行；Windows/macOS 平台 Gate 不变。这样 PR 仍能捕获平台专属链接、打包、libmpv 与安装器问题，移走的只是跨平台单元/集成测试的重复执行。

### D2：Task gates 拆成 4 个显式分片

用 `strategy.matrix.include` 定义四个 shard，每个 shard 显式列出 task id，不引入新的 runner 逻辑。分片按依赖与语言分组，避免每个 shard 都装 webkit / 构建前端。

| Shard | Tasks | 需要 Rust | 说明 |
|---|---|---|---|
| `core` | `1.1 1.3 1.8 3.1 4.1 5.1 6.1 11.5 12.5 13.3` | 是 | 先跑 cargo 重的 6.1/13.3/11.5/12.5，再跑静态与 1.3；12.5 的 benchmark 产物在此上传 |
| `desktop` | `7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2` | 是 | 7.1 先建立 `echo-desktop` 构建，11.4 复用；全部为 `echo-desktop` / 播放器范围 |
| `frontend-a` | `10.1 10.2 10.3 10.4 10.5 11.3 11.6 13.1` | 否 | 13.1 需要 Chrome；此 shard 设置 `CHROME_PATH` / `CHROME_NO_SANDBOX` |
| `frontend-b` | `10.7 10.8 10.9 10.10 12.1 12.2 12.3` | 否 | 纯前端；不安装 Rust，减少 setup |

- `16.1` 从 task-gates 移到 Rust Linux：它在完整工作区测试之后运行，`echo-app` 已构建，`cargo check` / `open_targets` 测试只需链接或复用，避免在 task-gates 里冷编译 shell。
- task-gates 不再安装 webkit、不再构建前端。当前 job 的注释声称 16.1/16.2 会编译 `echo-app`，实际只有 16.1 会；16.1 移走后该依赖消失。若后续新增 task 需要 shell，必须显式在该 shard 加 `webkit` / `frontend` 标志，而不是恢复全局构建。

### D3：Governance 按环境拆段，并把 cargo 依赖段迁到热构建 job

`ci-governance.mjs` 的检查表增加 `section` 字段，CLI 支持 `--sections=a,b`；未知 section 直接失败。默认（不传）运行全部 section，本地体验不变。

| Section | 检查 | CI 执行位置 |
|---|---|---|
| `quality` | fmt、clippy、workspace tests | 不通过 governance 跑；由 `rust` 矩阵覆盖（PR 三平台 fmt/clippy，Linux/non-PR 测试） |
| `registry` | reconcile、gen-scenario-manifests | `governance-static` |
| `rust-static` | validate-scenario-commands、scenario evidence | `rust-linux`，在完整测试之后（cargo 已热） |
| `static` | scale、fixture determinism、lint inheritance、toolchain pin、verification validity、scenario command proof | `governance-static` |
| `coverage` | task-12.8 `cargo llvm-cov` | `governance-static` |
| `purity` | build purity | `rust-linux`，`ECHO_PURITY_REQUIRE_BUILD=1` |
| `embedded` | embedded frontend freshness | `injection-shard-1`（shell 构建之后） |
| `injection` | injection suite | `injection-shard-1/2`（本地默认仍整跑） |

这样做的关键点是：`validate-scenario-commands` 与 `check-scenario-evidence` 都会 `cargo test --list`。它们当前没有 `--all-targets`，会额外构建 doc-tests（基线 323s，见 R5）；本 change 把调用改成与完整测试一致的 `--all-targets --all-features`，再放到 Rust Linux 的完整测试之后，它们只做列表与文件检查，成本降到几秒到几十秒。`coverage` 仍留在 governance-static，因为它自己用 `cargo llvm-cov` 构建，不需要 webkit 或前端。

`governance-static` 不再安装 webkit、不再构建前端；保留 ffmpeg（fixture 检查）与 coverage 工具。

### D4：Release gate 只跑场景，性能场景独立成 job

- `task-13.9.mjs` 新增 `--skip-static`：跳过 reconcile / validate / gen；`--delegated=` 继续透传给 `run-scenario.mjs`。
- Release gate 命令改为：

  ```text
  node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf
  ```

- 三个性能场景（`SFI-R05-S03`、`LE-R21-S02`、`PM-R13-S05`）在 `scenario-commands.mjs` 标记 `delegatedTo: "perf"`；新增 `package.json` 脚本：

  ```json
  "verify:perf": "node scripts/verify/run-scenario.mjs SFI-R05-S03 LE-R21-S02 PM-R13-S05"
  ```

- 新增 `perf` job：Rust + rust-cache + setup-node + pnpm（不装 webkit、不装 node 依赖、不构建前端），运行 `pnpm verify:perf`。三条场景共享同一条 `PERF_ALL` 命令，`run-scenario` 会去重为一次执行。
- `check-scenario-evidence.mjs` 的 `DELEGATED_GATES` 加入 `perf`；它用 `pnpm verify:perf` 在 `ci.yml` 中存在来证明该 gate 真的接了电。

**成本修正**：`--skip-static` 去掉的是重复的静态校验与 doc-test 构建，不是场景本身需要的编译。Release gate 的第一条场景命令仍要构建一次 per-package 测试二进制（按 Rust Linux 的全量 test 编译 231s 推算，echo-core/echo-desktop/echo-app 约 150–200s），随后场景复用。因此 Release gate 的预期是 **480–520s**（含 setup），不是 340s；`--skip-static` 的真实收益约 250s（去掉 doc-tests 与重复的 reconcile/validate/gen），而不是 490s。修正后的估算仍低于 10 分钟，但它是 PR 关键路径之一，必须按此验收。

### D5：注入证明拆成 2 个 shard

- 新增 `scripts/verify/injection-shard.mjs`，导出 `selectEntries(entries, { only, shard })`：按 `entry.shard ?? (index % total) + 1` 分配；`--shard=<i>/<n>` 选择第 i 片；非法 spec、越界、空片都失败。`selectEntries` 每次调用同时计算 1..n 各片的并集，断言并集等于全集且各片互斥；分片公式写错会让每个 shard 都红，而不是静默漏跑。
- `injection-suite.mjs` 只增加解析 `--shard=` 与调用 helper 的几行，保持文件在 1000 行以内（当前 987 行）。
- 嵌入前端新鲜度的注入条目显式 `shard: 1`，因为只有 shard 1 构建 shell 二进制；其余条目按稳定顺序轮转。
- 两个 shard 都做全量完整性检查（`ENTRIES` 全集），因此不会因为分片而漏掉「某检查没有证明」。
- shard 1 构建 shell 后额外运行一次正向的 `check-embedded-frontend.mjs`（`pnpm verify:frontend-fresh`），把这个检查的正向执行从 governance-static 移到这里；shard 2 不构建 shell，只构建前端（部分注入条目会 `cargo check -p echo-app`，需要 `dist`）。
- 两个 shard 都安装 Rust 工具链、rust-cache、webkit 与 pnpm 依赖：shard 2 的注入条目会 `cargo check -p echo-app`，Linux 上同样需要 webview 头文件；不安装会让条目在 suite 内假红。
- 分片只是选择执行哪些条目；`ECHO_INJECTION_REQUIRE_ALL=1` 与「全集完整性检查」在每一片都保留。

### D6：脚本级去重

1. **`self-test.mjs` 修复真实 cargo 泄漏**：最后一条 lockfile 不变性运行 `runRunner(fixture, ["ok", "failing", "--all"])` 时传入 `taskEnv`（fake cargo / fake pnpm 的 PATH），使 fixture 里的 `multi-name` / `missing-name` 不再调用真实 cargo。预计 1.3 从 171s 降到数秒。判定标准不变：self-test 仍须覆盖 unknown id、missing command、failing command、missing evidence、零测试、未观测证据、lockfile 不变。
2. **统一 feature / target 集**：task 检查中只针对 lib 模块的 cargo 调用加 `--all-features --lib`（`8.1`、`8.2`、`10.6`、`11.1`、`11.2`、`11.4`、`11.5`、`12.5`、`13.3`，以及 `task-6.mjs` / `task-7.mjs` 的循环）；`1.8` 保留 `--test logging`（集成测试，不能用 `--lib`）；`16.2` 已有 `--all-features --lib --locked`。这样同一 shard 内首次编译后，后续检查复用同一测试二进制。
3. **LFS 只留在 Windows**：仓库唯一的 LFS 对象是 `vendor/libmpv/windows/libmpv-2.dll`。Linux/macOS 的 checkout 去掉 `lfs: true`；Windows rust job 与 Windows platform Gate 保留。macOS 的 vendored dylib 不是 LFS。
4. **`--skip-static` / `--sections` 只改变执行位置**：不改变任何检查的断言；本地默认仍执行全集。
5. **对齐 `cargo test --list` 的构建形态**：`validate-scenario-commands.mjs` 与 `check-scenario-evidence.mjs` 的 `cargo test -p <pkg> --all-features -- --list` 改为 `cargo test -p <pkg> --all-targets --all-features -- --list`，与 Rust Linux 的完整测试步骤一致。仓库没有 doctest（`rg '^\s*///\s*```'` 为 0），`--all-targets` 不丢 doc 测试；去掉的是 R5 所述的无意义 doc-test 构建。若某个 package 的 bench/example 在 `--list` 下不可用，退回 `--lib --tests --all-features`（同样跳过 doc-tests），并保持与测试步骤同构。
6. **section 全覆盖守卫**：`ci-governance.mjs` 增加 `--list-sections`，`task-1.5` 把 `ci.yml` 中所有 `--sections=` 选择与 injection/embedded 的直接调用合并，断言每个 section 至少被执行一次。新增 section 而没有接线时，`task-1.5` 直接失败（见 §6、§7.3）。

### D7：前端矩阵保持不变

`frontend` 三平台 job 继续跑 format/lint/typecheck/test/build。它只占 70–80s，不是瓶颈；削减它会违反工程治理规格的稳定平台语义。

### D8：Linux platform Gate 并入 Rust (Linux)

`linux-platform-gate` 当前只跑 `pnpm verify:task -- 9.7 9.8`，基线 26s（校验步 1s）。把这两项追加到 Rust (Linux) 的测试后步骤，删除独立 job；断言内容不变，只是不再需要独立 runner。Rust (Linux) 的步骤顺序固定为：

```text
fmt → clippy → Test → Rust static gates → Task 16.1 → Task 9.7 9.8
```

Windows / macOS platform Gate 不变。

## 5. 目标 CI 结构

PR 上共 18 个 job（矩阵展开后）。队列并发 18 < GitHub Free 的 20 个并发上限，留 2 个余量。

| # | Job | 事件 | Runner | 关键步骤 | 预计墙钟 |
|---|---|---|---|---|---:|
| 1 | Rust (Linux) | PR + main | ubuntu | fmt、clippy、全量 test、`--sections=rust-static,purity`、16.1、9.7/9.8 | 480–520s |
| 2 | Rust (macOS) | PR + main | xcode-27 | fmt、clippy；PR 上 8.12/13.2；non-PR 加全量 test | 325s（PR）/ 810s（main） |
| 3 | Rust (Windows) | PR + main | windows | fmt、clippy；non-PR 加全量 test | 120s（PR）/ 955s（main） |
| 4–6 | Frontend (Linux/macOS/Windows) | PR + main | 三平台 | format/lint/typecheck/test/build | 70–80s |
| 7 | Task gates (core) | PR + main | ubuntu | 10 个 task + benchmark artifact | 300s |
| 8 | Task gates (desktop) | PR + main | ubuntu | 17 个 task | 350s |
| 9 | Task gates (frontend-a) | PR + main | ubuntu | 8 个 task，含 13.1 Chrome | 275s |
| 10 | Task gates (frontend-b) | PR + main | ubuntu | 7 个 task | 255s |
| 11 | Dependency audit | PR + main | ubuntu | 1.7 | 38s |
| 12 | macOS platform Gate | PR + main | xcode-27 | 1.9/1.10/9.7 | 306s |
| 13 | Windows platform Gate | PR + main | windows | 原生确认 + 9.7/9.8/17.1/18.1 | 313s |
| 14 | Governance static | PR + main | ubuntu | `--sections=registry,static,coverage` | ~390s |
| 15 | Injection proof 1/2 | PR + main | ubuntu | 前端构建、shell 构建、`--shard=1/2`、正向 embedded 检查 | ~390s |
| 16 | Injection proof 2/2 | PR + main | ubuntu | 前端构建、`--shard=2/2` | ~310s |
| 17 | Release gate | PR + main | ubuntu | `task-13.9 --skip-static --delegated=governance,perf` | 480–520s |
| 18 | Performance scenarios | PR + main | ubuntu | `pnpm verify:perf` | ~260s |

PR 关键路径预计是 Rust (Linux) 与 Release gate 的 480–520s，其次是 Governance static / Injection 1 / Task gates (desktop) 的 350–390s。加 10–30s 的 job 启动与排队，PR 预计 **8.5–9.8 分钟**。10 分钟作为验收上限，而不是典型值。

main 的墙钟由 Rust (Windows) 的 ~955s 决定，预计 16 分钟左右，比现在的 19–21 分钟更低；这是并行分片带来的附带收益，不是本 change 的验收指标。

### 事件条件

- `push: branches: [main]` 与 `pull_request` 保持不变。
- `Test` 步骤是唯一按事件分层的步骤；其余 job 在 PR/main 都执行。
- `concurrency` 保持 `cancel-in-progress: true`，避免同一 ref 的旧 run 占并发。
- `main` 的分支保护当前没有 required status checks（`required_status_checks: null`，`enforce_admins: true`），job 改名或合并不会卡住合并；反过来，CI 红也不会自动阻止合并。验收必须以 run 的 job/step 实跑记录为准，不能以「required check 变绿」代替。

### 平台条件（矩阵 job 的 Linux 专属步骤）

`rust` 与 `frontend` 是按 `matrix.platform` 展开的矩阵 job。任何只为 Linux 增加的步骤都必须用 **`if: matrix.platform == 'Linux'`** 收窄；`if: runner.os == 'Linux'` 在矩阵项里语义等价，但 `matrix.platform` 与 job 名、分片契约一致，统一用它：

| 步骤 | `if` | `run` |
|---|---|---|
| `Test` | `github.event_name != 'pull_request' \|\| matrix.platform == 'Linux'` | `cargo test --workspace --all-targets --all-features` |
| `Rust static gates` | `matrix.platform == 'Linux'` | `pnpm verify:governance -- --sections=rust-static,purity` |
| `Task 16.1` | `matrix.platform == 'Linux'` | `pnpm verify:task -- 16.1` |
| `Task 9.7 9.8` | `matrix.platform == 'Linux'` | `pnpm verify:task -- 9.7 9.8` |

`Rust static gates` 步骤同时设置 `env: ECHO_PURITY_REQUIRE_BUILD: "1"`。`governance-static`、`perf`、`release-gate` 与两个 injection shard 都是单平台 job，不需要此条件。

缺少条件会让 Linux 专属检查在 Windows/macOS 上重复执行，或在缺少 `llvm-cov` / `ffmpeg` 的 runner 上直接假红；两者都会浪费墙钟。`task-1.5` 对上述 `if`、`run`、`env` 做结构断言（见 §6）。

## 6. 脚本契约变更

### `scripts/verify/ci-governance.mjs`

- 检查表从 `[label, command, args]` 变为 `[label, section, command, args]`。
- 新增 `--sections=<逗号分隔>`；未知名失败；空选择失败；默认全部。
- 新增 `--list-sections`：每行打印一个 section 名（稳定排序），供 `task-1.5` 断言 CI 覆盖；与执行选择互斥。
- 保留 `--without-injection` 作为兼容别名（等价于从选择中移除 `injection`）。两个 flag 可同时出现：先按 `--sections` 选择，再移除 `injection`；若结果为空则失败。
- 输出中打印 `sections: <selected>/<all>`，使 CI 日志能看出跑了哪些段。
- 不改变任何检查的退出码语义；任一检查失败仍立即退出非零。

### `scripts/verify/run-scenario.mjs`

- `--delegated=` 支持逗号分隔的多个 gate（如 `governance,perf`）。
- 内部从单值比较改为集合判断；输出 `covered by the <gate> job` 时使用该场景声明的 gate。
- 未声明 `delegatedTo` 的场景仍然执行；未知 gate 名称不特殊处理（它只是匹配不到）。
- 汇总行保持 `scenario batches: N commands for M scenarios (K delegated)`。

### `scripts/verify/checks/task-13.9.mjs`

- 解析 `--skip-static`：跳过步骤 1 的三方对账、步骤 2 的 validate/gen；仍读取 manifest 计算 `expectedAutomatedCount`，仍执行场景与 `prd-matrix.mjs`。
- 带 `--skip-static` 时，最终输出必须明确写「static reconciliation delegated to governance-static/rust-static」，且不得再声称 `specs == traceability == manifest` 已验证；退出码仍由场景与 PRD matrix 决定。
- 失败信息明确说明静态校验已委托给哪些 job，避免「少跑了却看不出来」。
- 不带该 flag 时行为与现在完全一致。

### `scripts/verify/validate-scenario-commands.mjs`

- `cargoTestList` 的 `cargo test -p <pkg> --all-features -- --list` 改为 `cargo test -p <pkg> --all-targets --all-features -- --list`。
- 断言逻辑不变：仍然按 scenario 的命令声明得到 package 集合，并断言每个 rust evidence target 至少匹配该集合中的一个测试。
- 失败信息不变，便于注入证明与日志检索。

### `scripts/verify/check-scenario-evidence.mjs`

- `cargoTestList` 同样改为 `--all-targets --all-features`。
- `DELEGATED_GATES` 从 `{"governance"}` 变为 `{"governance", "perf"}`。
- 仍要求 `ci.yml` 中存在 `pnpm verify:<gate>`。
- 证据数量下限不变（523 场景 / 541 证据）；本 change 不删证据。

### `scripts/verify/scenario-commands.mjs` 与 `manifest.json`

- 三个性能场景加 `delegatedTo: "perf"`。
- `manifest.json` 由 `node scripts/verify/gen-scenario-manifests.mjs --write` 重新生成，不手工编辑。
- `checks` / `evidence` 不变。

### `package.json`

- 新增 `verify:perf`，命令见 D4。
- 其余脚本不变。

### `scripts/verify/self-test.mjs`

- 最后一条 lockfile 不变性运行使用 `taskEnv`。
- 增加一个 `delegatedTo: "perf"` 的 fixture 场景，并用 `--delegated=governance,perf` 断言两个 gate 都被识别、未声明的 Rust 场景仍执行。
- 现有单 gate 断言保留，防止多 gate 改造破坏兼容性。

### `scripts/verify/checks/task-1.5.mjs`

`task-1.5` 是 CI 形状的结构守卫。它不实跑各 job 的命令，只解析 `ci.yml` 的 YAML 并断言结构，因此新增断言不增加 CI 墙钟。

- `REQUIRED` 中的测试命令改为 `cargo test --workspace --all-targets --all-features`。
- 新增一个按 job/step 名查找步骤的 helper，并逐项断言：

| Job | Step | 断言 |
|---|---|---|
| `rust` | `Test` | `run` 等于新命令；`if` 同时包含 `github.event_name != 'pull_request'` 与 `matrix.platform == 'Linux'`；`continue-on-error` 不为 true |
| `rust` | `Rust static gates` | `run` 等于 `pnpm verify:governance -- --sections=rust-static,purity`；`if` 包含 `matrix.platform == 'Linux'`；`env.ECHO_PURITY_REQUIRE_BUILD` 等于 `"1"` |
| `rust` | `Task 16.1` | `run` 等于 `pnpm verify:task -- 16.1`；`if` 包含 `matrix.platform == 'Linux'` |
| `rust` | `Task 9.7 9.8` | `run` 等于 `pnpm verify:task -- 9.7 9.8`；`if` 包含 `matrix.platform == 'Linux'` |
| `governance-static` | 任意步骤 | 存在 `run` 等于 `pnpm verify:governance -- --sections=registry,static,coverage` |
| `perf` | 任意步骤 | 存在 `run` 等于 `pnpm verify:perf` |
| `release-gate` | 任意步骤 | 存在 `run` 等于 `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf` |
| `injection-proofs` | — | `matrix.include` 恰有两项（`shard: 1`、`shard: 2`），且某步骤 `run` 含 `--shard=${{ matrix.shard }}/2` |
| `task-gates` | — | 各 shard 的 task 列表并集等于固定 **42 项**清单（43 项减去移到 Rust Linux 的 `16.1`），无重复、无 archived id；与 `rust` job 的 `Task 16.1` 步骤合起来恰好覆盖 43 项 |

- 固定 43 项清单就是当前 `ci.yml` 中的 task 列表（`1.1 … 16.2`，含 16.1），在 `task-1.5` 里作为常量维护；其中 `16.1` 由 Rust Linux 的步骤覆盖，其余 42 项由 4 个 shard 覆盖。它防止分片改造漏掉某个 task；未来新增 task 仍沿用现有的人工登记方式。
- **section 全覆盖**：`task-1.5` 运行 `node scripts/verify/ci-governance.mjs --list-sections` 取得权威 section 列表，并按下面的映射断言每一项都在 `ci.yml` 中有执行位置：

  | Section | CI 执行位置断言 |
  |---|---|
  | `quality` | `rust` job 的 fmt / clippy / test 命令（`REQUIRED` 已覆盖） |
  | `registry`、`static`、`coverage` | `governance-static` 的 `--sections=registry,static,coverage` |
  | `rust-static`、`purity` | `rust` job 的 Linux 步骤 `--sections=rust-static,purity` |
  | `injection` | 两个 injection shard 的 `node scripts/verify/injection-suite.mjs --shard=...` |
  | `embedded` | injection shard 1 的 `pnpm verify:frontend-fresh` |

  若 `--list-sections` 出现映射表外的 section，或映射表里的 section 不在输出中，`task-1.5` 失败。这样新增 section 而不接线、或从某个 `--sections=` 里删掉一段，都会红。
- 失败信息必须区分「job 不存在」「step 不存在」「run 不匹配」「if 条件不匹配」「env 不匹配」，便于注入证明精确匹配，也便于作者定位。
- 不新增独立检查文件：`task-1.5` 已经由 `injection-governance.mjs` 覆盖，扩展它比新增一个必须补注入证明的检查更省维护。

### `scripts/verify/injection-shard.mjs` 与 `injection-suite.mjs`

- 新 helper 的 API 与失败模式见 D5。
- `selectEntries` 在每次调用时验证 1..n 各片的并集等于全集、各片互斥；这是分片公式的运行时守卫，不依赖额外测试文件。
- `injection-suite.mjs` 只加 import、解析 `--shard=`、调用 helper；文件保持 < 1000 行。
- 嵌入前端条目加 `shard: 1`。
- `--list` 仍列出全部条目，不受 shard 过滤。

### `scripts/verify/injection-governance.mjs`

新增三条注入证明，均针对 `task-1.5` 的新结构断言：

1. 把 `Test` 条件改成 `matrix.platform == 'Windows'`，期望 `task-1.5.mjs` 以「Test 步骤条件未覆盖 Linux PR」失败。
2. 删除 `Rust static gates` 步骤（或把它的 `run` 改成别的命令），期望 `task-1.5.mjs` 以「rust-static 委托调用缺失」失败。
3. 从 `governance-static` 的 `--sections=` 中删掉 `coverage`，期望 `task-1.5.mjs` 以「section coverage 未接线」失败。

第二条是 §7.3 所述结构守卫的证明：如果没有它，`task-1.5` 新增的断言可能退化成恒真，`rust-static` 的 CI 接线仍可能被静默删除。第三条证明 section 全覆盖守卫不是恒真。现有 `ci-platform-matrix/missing-linux-entry` 条目保留。

### Task 检查的 flag 对齐

| 文件 | 变更 |
|---|---|
| `task-8.1.mjs` | `cargo test -p echo-desktop --lib` → 加 `--all-features` |
| `task-8.2.mjs` | 同上 |
| `task-10.6.mjs` | `cargo test -p echo-desktop <filter>` → 加 `--all-features --lib` |
| `task-11.1.mjs` | 同上 |
| `task-11.2.mjs` | 同上 |
| `task-11.4.mjs` | 三个 cargo 调用加 `--all-features --lib` |
| `task-11.5.mjs` | `cargo test -p echo-core <filter>` → 加 `--all-features --lib` |
| `task-12.5.mjs` | bench 调用加 `--lib` |
| `task-13.3.mjs` | 加 `--lib`（已有 `--all-features`） |
| `task-6.mjs` / `task-7.mjs` | 循环内 cargo 调用加 `--lib` |
| `task-1.8.mjs` | 不变（`--test logging` 是集成测试） |
| `task-16.2.mjs` | 不变（已有 `--all-features --lib --locked`） |

只对确定是 lib 模块的过滤目标加 `--lib`；加之前逐个确认过滤名在 `cargo test --lib -- --list` 中存在。若某个目标实际是集成测试，保留原形态。

### 文档

- `docs/arch-standards-remaining.md`：更新 CI job 拓扑、governance 分段、PR/main 覆盖分层、`verify:perf`、injection 分片。
- `docs/ci-pipeline-review-2026-10-28.md`：追加本次改造的结果与实测墙钟（若该文档的定位是历史 review，则只追加「后续」小节，不改历史结论）。
- OpenSpec：新增 change `ci-pr-under-10-min`，delta 修改 `engineering-governance`，至少覆盖：
  - PR 快反馈层与 non-PR 全平台测试层的分工；
  - 每个门禁必须在 CI 的某个事件中被执行，委托必须在 CI 中有对应命令；
  - 缩短 CI 的改造只能迁移/分片/复用，不能删除检查或放宽断言；
  - `Test` 条件的结构守卫与其注入证明。

## 7. 覆盖映射与「不静默」保证

### 7.1 检查去向

| 检查 | PR 执行位置 | main 执行位置 |
|---|---|---|
| fmt / clippy | 三平台 Rust job | 三平台 Rust job |
| 完整 workspace tests | Rust (Linux) | 三平台 Rust job |
| 8.12 / 13.2 | Rust (macOS) | Rust (macOS) |
| 1.9/1.10/9.7 | macOS platform Gate | macOS platform Gate |
| 9.7/9.8/17.1/18.1 + 原生确认 | Windows platform Gate | Windows platform Gate |
| 9.7/9.8（Linux） | Rust (Linux) | Rust (Linux) |
| 43 个 task | 4 个 task-gates shard（16.1 在 Rust Linux） | 同 PR |
| reconcile / gen | Governance static（`registry`） | 同 PR |
| validate / scenario evidence | Rust (Linux)（`rust-static`） | Rust (Linux) |
| scale / fixture / lint / toolchain / validity / command proof | Governance static（`static`） | 同 PR |
| coverage | Governance static（`coverage`） | 同 PR |
| build purity | Rust (Linux)（`purity`，`ECHO_PURITY_REQUIRE_BUILD=1`） | Rust (Linux) |
| embedded frontend freshness | Injection shard 1（正向）+ 注入条目 | 同 PR |
| injection proofs | Injection shard 1/2 | 同 PR |
| 自动化场景 | Release gate（governance/perf 场景委托） | 同 PR |
| 性能场景 | Performance job（`perf` 委托） | 同 PR |
| PRD matrix | Release gate | 同 PR |

没有检查只在本地运行；`quality` section 只在本地默认聚合中执行，但它的三条命令在 CI 的 Rust 矩阵中各自执行，满足「核心门禁集在 CI 中执行」。

### 7.2 委托的真实性

- `governance` 委托现在的含义是「由 Governance static + Rust (Linux) 的 `rust-static`/`purity` + Injection shard 1 的 embedded 正向检查共同覆盖」。`check-scenario-evidence.mjs` 仍要求 `pnpm verify:governance` 出现在 `ci.yml`；本 change 中它出现在 `governance-static` 与 `rust-linux` 的 `--sections=` 调用里。
- `perf` 委托要求 `pnpm verify:perf` 出现在 `ci.yml`；新增的 performance job 满足。
- `run-scenario` 只跳过注册表里显式声明 `delegatedTo` 的场景；不会因为命令形状或 job 名称推断覆盖。

### 7.3 防退化

- `ci-governance.mjs` 对未知 section 失败；空选择失败；`--sections` 语法错误失败。
- `task-1.5` 对 `Test`、`Rust static gates`、`Task 16.1`、`Task 9.7 9.8` 的 `run` / `if` / `env` 做结构断言，并断言 `governance-static`、`perf`、`release-gate`、injection 分片和 task 分片的接线存在。
- `task-1.5` 用 `ci-governance.mjs --list-sections` 的权威列表做 section 全覆盖断言；新增 section 未接线或从 `--sections=` 删除一段都会失败。
- 新增三条注入证明：把 `Test` 条件改成 Windows、删除 `Rust static gates`、从 governance-static 删除 `coverage`；三者都必须让 `task-1.5` 失败。第二条专门防止 `rust-static` 的 CI 调用被静默移除，第三条防止 section 守卫恒真。
- `check-scenario-evidence` 的证据下限不变；`check-scenario-command-proof` 的注入证明登记随新增条目同步更新。
- injection 分片的完整性检查使用全集 `ENTRIES`，每个 shard 都验证「所有自包含检查都有注入条目」。
- `check-scale` 继续限制 1000 行；新增 helper 与文档不触碰上限。

## 8. 预期收益

| 指标 | 现状 | 目标 |
|---|---:|---:|
| PR 墙钟 | 21m46s | 8.5–9.8 min，验收上限 10 min |
| PR 上最慢 job | Task gates 1304s | Rust (Linux) / Release gate 480–520s |
| main 墙钟 | 19m21s | ~16 min（附带收益，不承诺） |
| PR 全平台 fmt/clippy | 有 | 有 |
| PR Windows/macOS 完整测试 | 有 | 无（移到 main） |
| PR 平台 Gate | 有 | 有 |
| 检查/证据数量 | 523/541 | 不变 |

收益来源分解：

- Task gates 分片 + 1.3 修复 + 11.4 复用：1304s → 350s（最慢 shard），贡献约 950s。
- Governance 去重 + 分段：1015s → 390s（static），贡献约 625s；`rust-static` 迁到热构建约 40s。
- Release gate 去静态 + 去 perf：827s → 480–520s，贡献约 320s（原先的 490s 估算高估了；静态预检中的测试二进制构建会转移到首条场景命令，见 D4/R5）。
- `cargo test --list` 对齐 `--all-targets`：把 rust-static 从 323s 降到几秒（本机热构建 0.9s），是 Rust Linux 不超时前提。
- Injection 分片：598s → 390s（最慢 shard），贡献约 208s。
- Windows/macOS 完整测试移出 PR：983s/830s → 120s/325s，贡献约 860s/505s（但它们原本不是唯一关键路径）。
- 前端矩阵不变。

## 9. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Rust (Linux) 与 Release gate 成为双关键路径（各 480–520s），冷缓存或 `--all-targets` 变慢会逼近 10 min | 验收失败 | rust-static 在测试之后复用构建；`cargo test --list` 统一 `--all-targets`（R5）；如果实测 > 10 min，下一杠杆是把 `rust-static` 移到 Release gate 的场景步骤之后（此时已热）或进一步减少 task shard 1 的串行项 |
| `validate-scenario-commands` / `check-scenario-evidence` 在 Rust Linux 上仍然慢 | Rust Linux 超时 | 已定位为 doc-test 构建（R5）：改成 `--all-targets --all-features` 后，本机热构建 0.9s；CI 日志会显示 `cargo test --list` 的耗时，若仍 > 60s 则回退到 `--lib --tests` |
| Release gate 去掉静态预检后，场景首条命令承担测试二进制构建 | 估算被低估 | 已在 D4 把 Release gate 修正为 480–520s，并把它列为 PR 关键路径；验收时单独记录第一条场景命令的耗时 |
| 冷缓存（rust-cache miss）下 Rust Linux 可能显著超过 10 min | 验收口径争议 | 验收以「rust-cache 命中的代表性 PR run」为主口径；冷缓存 run 单独记录，若 > 10 min 不作为本 change 的失败，但必须写入文档并评估 main 预热 |
| 18 个 job 接近并发上限，排队增加 | PR 墙钟抖动 | 按 GitHub Free 公共仓库标准 runner 20 并发（macOS 5）设计，18 < 20；`concurrency` 取消旧 run。该上限是外部平台事实，无法从仓库静态确认；首次 PR 实测若出现排队，优先合并 dependency-audit 到 governance-static（38s） |
| `task-1.5` 的精确 `run` / `if` 断言与 CI 文本强耦合 | 以后改命令要同步改检查 | 断言按 job/step 名定位并给出精确失败原因；这是仓库既有的「CI 形状反向断言」纪律，不是本 change 新引入的成本 |
| 新增的 Linux 专属步骤漏写 `matrix.platform == 'Linux'` | 在 Windows/macOS 假红或浪费墙钟 | `task-1.5` 逐步骤断言条件；注入证明覆盖 `Test` 条件；PR 实测看三平台日志 |
| task 分片遗漏某个 task | 检查静默消失 | `task-1.5` 断言 4 个 shard 的并集等于固定 42 项清单（16.1 由 Rust Linux 覆盖），且无重复 |
| `--all-targets` 增加 Linux 测试编译时间 | Rust Linux 变慢 | 与 governance 原覆盖一致；仓库无 doctest；实测后若超预算，可只保留 `--all-features` 并把 bench 编译留给 task-12.5 |
| self-test 用 fake cargo 后可能掩盖真实 cargo 的 lockfile 行为 | 1.3 检查变弱 | self-test 仍断言 lockfile SHA-256；真实 `pnpm install --frozen-lockfile` 仍在 1.3 中执行；fake cargo 只替代 fixture 的 task-4 调用 |
| injection-suite 接近 1000 行 | scale gate 红 | shard 逻辑放新 helper；suite 只加 import 与调用；必要时先拆出现有 ENTRIES 到新文件（单独的机械重构） |
| 注入 shard 2 缺少 shell 构建，某些条目需要 `dist` | 注入失败 | shard 2 仍构建前端；只有 `cargo build -p echo-app` 被跳过；嵌入前端条目显式 `shard: 1` |
| benchmark-50k artifact 只在 shard 1 | artifact 缺失 | 12.5 固定在 shard 1；upload 步骤只在该 shard；`if-no-files-found: error` 保持 |
| Windows/macOS 回归在 PR 上漏过，merge 后 main 才红 | 合并后返工 | 用户已接受；平台 Gate、clippy、8.12/13.2 仍在 PR；main 的完整测试是兜底；PR 模板/文档注明该分层 |
| macOS runner 并发限制（常见为 5） | 排队 | macOS 相关 job 只有 3 个（Rust macOS、Frontend macOS、macOS platform Gate），低于常见上限 |
| cache 失效导致首次运行超时 | 一次性红 | 主口径是 rust-cache 命中的第二次 run；冷缓存 run 单独记录，不作为失败条件（见 §11 裁定） |
| `--skip-static` 让 Release gate 看不到对账失败 | 发布门禁变弱 | 对账在 Governance static 的 `registry` 段执行；`check-scenario-evidence` 在 Rust Linux 执行；`task-13.9` 的失败信息写明委托目标；本地默认仍全跑 |

## 10. 验证与验收

### 本地验证（实现阶段）

1. `node scripts/verify/checks/task-1.3.mjs`：self-test 在 fake PATH 下通过，lockfile 不变。
2. `node scripts/verify/checks/task-1.5.mjs`：新 `Test` 命令、Linux 条件、section 全覆盖与 task 分片断言通过。
3. `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf`：场景执行与 PRD matrix 通过。
4. `node scripts/verify/run-scenario.mjs SFI-R05-S03 LE-R21-S02 PM-R13-S05`：性能场景可执行。
5. `node scripts/verify/check-scenario-evidence.mjs`：`perf` gate 被识别，证据下限不变。
6. 在 `cargo test --workspace --all-targets --all-features` 之后运行 `validate-scenario-commands.mjs` 与 `check-scenario-evidence.mjs`：两者合计应 < 60s（本机热构建应约 1–5s）。
7. `node scripts/verify/injection-suite.mjs --shard=1/2` 与 `--shard=2/2`：两片都通过，完整性检查覆盖全集。
8. `node scripts/verify/checks/task-16.1.mjs`：在 Linux 上通过。
9. `node scripts/verify/check-scale.mjs`：新文件不超限。
10. **独占运行** `pnpm verify:governance`：全部 section 通过（本机 5 秒墙钟断言要求不并行跑 cargo/rg/node 扫描）。

### CI 验证

1. 在分支上推一次 PR，记录 run 的总墙钟与各 job 墙钟。
2. 记录 Rust (Linux) 的 `Test`、`rust-static`、16.1、purity 步骤耗时；记录 Governance static、Injection 1/2、Release gate、Performance 的耗时。
3. 合并后记录 main run，确认三平台 `Test` 都执行（Windows/macOS 的 `Test` 步骤不是 skipped）。
4. 用 `gh run view <id> --json jobs` 生成对比表，写入 `docs/arch-standards-remaining.md` 或 `docs/ci-pipeline-review-2026-10-28.md` 的后续小节。

### 验收标准

- 代表性 PR run 的总墙钟 < 10 分钟。主口径是 **rust-cache 命中**的常态 run（连续两次同分支 run 取第二次）；冷缓存 run（rust-cache miss 或新缓存 key）单独记录，不作为本 change 的通过条件，但必须写入文档并评估增量。
- PR 上三平台 fmt/clippy、Linux 全量测试、Windows/macOS 平台 Gate 全部执行且为绿。
- main run 上 Windows/macOS 的 `Test` 步骤实际执行（非 skipped）。
- 523 场景 / 541 证据、注入证明数量、task 清单不减少；4 个 task 分片的 task 并集恰好是 42 项，与 Rust Linux 的 `16.1` 合起来恰好覆盖 43 项，每个 task 只出现一次。
- `ci-governance.mjs --list-sections` 的每个 section 都在 `ci.yml` 中有执行位置；`task-1.5` 的 section 全覆盖断言为绿。
- `pnpm verify:governance` 本地全绿。
- 没有检查从 PR 和 main 同时消失；`check-scenario-evidence`、`task-1.5`、`check-scale`、injection 完整性均为绿。
- 由于 `main` 没有 required status checks，验收以 `gh run view <id>` 的 job/step 记录为准；不能因为分支保护未拦截就认为检查已执行。
- 单独记录 Rust (Linux) 与 Release gate 的墙钟，以及 Release gate 第一条场景命令的耗时；两者都必须在 480–520s 的估算带内，否则触发回滚项。

### 回滚

如果实测 PR 墙钟仍 > 10 分钟，按以下顺序回退，而不是放宽断言：

1. 把 `rust-static` 从 Rust Linux 移到 Release gate 的**场景步骤之后**（此时测试二进制已由场景构建，rust-static 只需几秒；Rust Linux 减约 10s）。不要放在场景之前——Release gate 的 target 起始是冷的。
2. 把 task-gates `core` 的 `12.5` 移到 `frontend-b` 或独立 job（仅影响 benchmark artifact 位置）。
3. 合并 `dependency-audit` 到 `governance-static`，减少一个 job。
4. 仍不达标时，重新评估 Windows/macOS 的 `clippy` 是否需要在 PR 上三平台执行（用户已要求保留，默认不动）。
5. 若 Release gate 的实测超过 560s，把它拆成两个场景 shard（按 command 稳定分区，两个 shard 各自承担一次测试二进制构建）；这会让 job 数从 18 升到 19，仍在 20 并发以内。

## 11. 本次设计做出的裁定（供审阅）

以下不是用户提出的，是我在调查后做的判断；若不同意，改动其中一项不会推翻整体设计：

- **冷缓存不作为验收失败条件**：主口径是 rust-cache 命中的常态 PR run；冷缓存 run 单独记录。理由：冷缓存下依赖编译是平台成本，不是本 change 能消除的检查开销。
- **`cargo test --list` 用 `--all-targets` 对齐完整测试构建**，而不是引入共享清单 helper（JSON `--message-format` + 直接执行测试二进制）。后者也能把成本降到几秒，但复杂度和风险更高；本机实测 `--all-targets -- --list` 已足够快。
- **Release gate 保持独立 job，接受 480–520s**；不预先拆成 2 个 shard。它仍在 10 分钟以内，且 18 个 job 比 19–20 个更安全；拆 shard 列为回滚项 5。
- **两个 injection shard 都安装 webkit/Rust**，而不是把 `cargo check -p echo-app` 的条目钉到 shard 1。理由：分片公式保持简单，两个 shard 的 wall clock 都远低于关键路径。
- **section 全覆盖由 `task-1.5` + `ci-governance.mjs --list-sections` 实现**，不新增独立检查文件，避免再补一套注入证明。

## 12. 交给构建者的决定

以下不改变「做什么」，只影响实现细节，由构建者在计划阶段裁定：

- `ci-governance.mjs` 的 section 命名（`registry` / `rust-static` / `static` / `coverage` / `purity` / `embedded` / `injection` / `quality`）可以按可读性微调，但不得合并到无法表达「在哪个 job 执行」的程度。
- task shard 内部的精确排序可按实测调整；约束是：cargo 重的任务先跑、同一 feature 集的任务相邻、12.5 留在 core shard。
- injection 分片的轮转顺序可调整；约束是嵌入前端条目必须在构建 shell 的 shard。
- 是否把 `dependency-audit` 并入 `governance-static`，可在实测 job 数/排队后决定；默认保持独立。
- 是否在 `perf` job 里运行 `pnpm install`：默认不运行（`run-scenario.mjs` 只用 Node 内置模块），若 pnpm 脚本解析需要则再加。

以下必须由用户/规格决定，构建者不得自行改变：

- PR 上 Windows/macOS 不跑完整工作区测试（已批准）。
- 前端三平台矩阵保留（规格要求）。
- 任何检查的断言强度不得降低；只能迁移、分片、复用。
