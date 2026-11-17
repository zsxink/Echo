# Tasks

## 1. rust 双 job 共 5 分片、步骤重排、purity 迁出

- [ ] 1.1 把 rust 分片拆为两个 job：`rust` 承载 3 个 include 条目（Linux `workspace`、macOS `core`、Windows `core`），**无 job 级 `if`**；新增 `rust-desktop` 承载 2 个 include 条目（macOS `desktop`、Windows `desktop`），job 级 `if: github.event_name != 'pull_request'`（只含 `github` 上下文，合法）。两个 job 的矩阵条目均包含 `platform` / `shard` / `os` / `cache` / `packages` / `clippy`，并都用 `shared-key: ${{ matrix.cache }}`。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0，且两个 job 的 `cargo test ${{ matrix.packages }} --all-targets --all-features` 包并集等于全部 workspace 包。
- [ ] 1.2 重排整个 `rust` job（含 Linux `workspace` 分片）的步骤为 Format check → Task 16.1 → Task 9.7/9.8 → Clippy → Test → Rust static gates，保证 `Test` 是该 job 最后一次全量编译；`Task 16.1`、`Task 9.7/9.8`、`Rust static gates` 限定 `matrix.shard == 'workspace'`，`Clippy` 按 `matrix.clippy` 执行；`rust-desktop` 只复制 `Test` 及其前置 setup 步骤（`Test` 无 `if`，`run` 与 `rust` 完全一致），不复制 `Task 16.1` / `Task 9.7/9.8` / `Rust static gates` / `Clippy`；把 `purity` section 从 `Rust (Linux)` 迁到 `governance-static`（`ECHO_PURITY_REQUIRE_BUILD=1`，位于 coverage 之前）。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；`node scripts/verify/checks/task-16.1.mjs`、`task-9.1.mjs` 与 `task-9.7`/`task-9.8` 相关检查退出 0。
- [ ] 1.3 新增 `macos-native-gate` job（插在 `macos-platform-gate` 之后），承接从 `rust` job 迁出的 8.12/13.2 libmpv smoke 与 native e2e 门禁；该 job 自行 stage vendored libmpv dylibs 到 `target/Frameworks`，只运行 `pnpm verify:task -- 8.12 13.2`，不执行 `pnpm install`；同时更新 `ci.yml` 的 `task-gates` 注释中关于 8.12/13.2 执行位置的过时说明。验证：`node scripts/verify/checks/task-1.5.mjs` 的 `assertAlwaysRuns` 覆盖 `macos-native-gate` 且退出 0；`rg -n '8.12 13.2' .github/workflows/ci.yml` 只命中新 job。

## 2. 依赖审计分层、删除 dependency-audit job、新增定时 workflow

- [ ] 2.1 删除 `ci.yml` 的 `dependency-audit` job，把 `pnpm verify:task -- 1.7` 并入 `governance-static`；在线 RUSTSEC 公告扫描按事件分层：PR 只跑离线半，`main` 跑在线半，PR 跳过在线半时打印显式 note；同步更新 `ci.yml` 中关于 1.7 执行位置的过时注释（`dependency-audit job` → `governance-static job`）。验证：`node scripts/verify/checks/task-1.7.mjs` 退出 0；`rg -n 'dependency-audit' .github/workflows/ci.yml` 无输出。
- [ ] 2.2 新增 `.github/workflows/dependency-audit.yml`，仅含 `schedule` 与 `workflow_dispatch` 触发，1 个 job 真实执行在线扫描。验证：`rg -n 'schedule|workflow_dispatch|task-1.7' .github/workflows/dependency-audit.yml` 命中三项；结构门禁断言该 workflow 存在。
- [ ] 2.3 在 `task-1.5.mjs` 增加静态断言：读取 `scripts/verify/checks/task-1.7.mjs` 源码，要求同时包含 `ECHO_AUDIT_OFFLINE_ONLY` 与 note 文案 `1.7: note — online advisory checks`，使跳过 note 的存在性不再依赖人工检查。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；临时删除 note 文案后该检查以非零退出并报告缺失的 needle。

## 3. 缓存治理（shared-key + save-if）

- [ ] 3.1 删除现存 14 条 Actions 缓存（`gh api -X DELETE`），避免旧配额继续触发淘汰。验证：`gh api repos/zsxink/Echo/actions/cache/usage` 的 `active_caches_count` 归零或仅剩本次实施后的新缓存。
- [ ] 3.2 为所有 `Swatinem/rust-cache` 步骤统一 `shared-key`（`rust` 与 `rust-desktop` 两个 job 各自用 `${{ matrix.cache }}` 展开，合计 5 个 key；`governance-static` 用 `governance-static`），写者集合恰好 6 个 key（`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`）；`save-if: ${{ github.ref == 'refs/heads/main' }}` 只出现在 `rust`、`rust-desktop`、`governance-static` 三个 job，其余 job 全部 `save-if: false`。验证：`node scripts/verify/checks/task-1.5.mjs` 的缓存写者集合断言退出 0；`rg -n 'save-if' .github/workflows/ci.yml` 的 main 条件只命中这三个 job。
- [ ] 3.3 在 `task-1.5.mjs` 增加 `governance-static` key 的读写者断言：(a) 遍历 `ci.yml` 各 job 的缓存步骤，`shared-key === "governance-static"` 的 job 集合必须恰为 `["governance-static"]`，任何其他 job（例如 `task-gates`）指向该 key 都必须失败；(b) 解析 `.github/workflows/dependency-audit.yml`，其缓存步骤的 `shared-key` 必须恰为 `governance-static`。这两条断言与 3.2 的写者集合共同支撑「`governance-static` 的 job 私有缓存不受形态约束」场景。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；临时把 `task-gates` 的 `shared-key` 改为 `governance-static` 后检查以非零退出，临时改掉 `dependency-audit.yml` 的 `shared-key` 后同样失败。

## 4. task-gates 4 → 3 分片

- [ ] 4.1 把 `task-gates` 的 `frontend-a` 与 `frontend-b` 合并为 `frontend`，保留 `core` 与 `desktop`；三片并集仍覆盖全部 42 项 task 且无重复。验证：`node scripts/verify/checks/task-1.5.mjs` 的分片并集断言退出 0；`rg -n 'frontend-a|frontend-b' .github/workflows/ci.yml scripts/verify` 无输出（新分片计划文件除外）。

## 5. 注入证明 2 → 3 分片

- [ ] 5.1 把注入证明拆为 3 片，按实测成本显式标注 `shard:`；嵌入前端条目固定在构建 shell 的分片（shard 1）；分片计划放入新文件以避开 `injection-suite.mjs` 的 1000 行上限；同步 `injection-governance.mjs` 与 `task-1.5.mjs` 的分片集合为 `[1, 2, 3]`。验证：`node scripts/verify/injection-suite.mjs --shard=1/3`、`--shard=2/3`、`--shard=3/3` 各自退出 0；`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [ ] 5.2 断言分片计划无空片、无重叠：计划键集等于注入套件 id 集。验证：`node scripts/verify/injection-suite.mjs --shard=1/3` 等三片输出无空片；`node scripts/verify/check-scale.mjs` 退出 0。

## 6. 场景命令与任务脚本的 cargo 形态收敛

- [ ] 6.1 在 `run-scenario.mjs` 的 runner 层（`groupedTestCommand`）把无额外参数的按包场景 cargo 命令归一化为 `cargo test --workspace --all-targets --all-features`；注册表保留按包原始命令，`validate-scenario-commands` 语义不变。验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0；`node scripts/verify/run-scenario.mjs -- --automated` 场景与证据数不变（561/561）。
- [ ] 6.2 给 `task-9.1.mjs` 的 `cargo check` / `cargo test` 与 `task-16.1.mjs` 的 `cargo check` 补 `--all-features`（`task-9.1` 的 `--lib` 保留）；把 `task-12.4.mjs` 的三次独立 vitest 合并为一次多文件调用；注入证明 shell 构建改为 `cargo build -p echo-app --all-features`。验证：`node scripts/verify/checks/task-9.1.mjs`、`task-16.1.mjs`、`task-12.4.mjs` 退出 0；`node scripts/verify/injection-suite.mjs --shard=1/3` 退出 0。

## 7. 重写 ci-structure 注入证明

- [ ] 7.1 在 `injection-governance.mjs` 按新结构重写 ci-structure 注入证明：删除 matrix 分片、把 clippy 移到 `Test` 之后、把 `purity` 从 `governance-static` 删除、把 injection 分片数改回 2，四类注入都必须让 `task-1.5` 失败；证明总数由 12 条增至 13 条，新增 `ci-structure/job-if-references-matrix`（把 `rust-desktop` 的 job 级 `if` 改为引用 `matrix.shard` 时必须失败），并把新 id 登记进注入分片计划。验证：`node scripts/verify/injection-governance.mjs` 退出 0，且各注入产生预期的失败信息；`node scripts/verify/injection-suite.mjs --only ci-` 全部通过。
- [ ] 7.2 同步 `task-1.5.mjs` 的 `assertAlwaysRuns` 覆盖新 job id、`purity` section 映射，以及「任何 job 级 `if` MUST NOT 引用 `matrix.`」的永久断言。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；`rg -n 'purity|alwaysRuns|must not reference matrix' scripts/verify/checks/task-1.5.mjs` 命中新映射与新断言。

## 8. 本地全量验证与耗时测量

- [ ] 8.1 本地按分片各跑一遍测试，确认无隐式顺序依赖；在 `cargo test --workspace --all-targets --all-features` 之后运行两个 validator，合计 < 30s。验证：各分片命令退出 0；validator 耗时记录 < 30s。
- [ ] 8.2 **独占**运行 `pnpm verify:governance`（不与 cargo、node 扫描或 rg 并行，避免 5 秒墙钟断言被干扰）。验证：整门退出 0，注入自证全部通过；失败只修本 change 引入的问题，既有 flake 如实记录。
- [ ] 8.3 记录 PR run 的总墙钟与每个 job 墙钟，确认 < 10 分钟；合并后确认 `main` 的 5 个 rust 分片全部执行（`desktop` 不得 skipped）、连续两次 run 的缓存主键命中且 Test 编译数 < 50 个 crate、缓存总量 < 6 GB。验证：`gh run view` 与 `gh api repos/zsxink/Echo/actions/cache/usage` 的结果写入 PR 正文。
