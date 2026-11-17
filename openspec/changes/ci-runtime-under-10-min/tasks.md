# Tasks

## 1. rust 矩阵 5 分片、步骤重排、purity 迁出

- [ ] 1.1 把 `rust` 矩阵改为 5 个 include 条目：Linux `workspace`、macOS `core` / `desktop`、Windows `core` / `desktop`；`desktop` 分片 job 级仅在非 `pull_request` 执行，`core` 分片的 `Test` 按事件与分片设置 `if`；矩阵条目包含 `platform` / `shard` / `os` / `cache` / `packages` / `clippy`。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0，且 `cargo test ${{ matrix.packages }} --all-targets --all-features` 的包并集等于全部 workspace 包。
- [ ] 1.2 重排 `Rust (Linux)` 步骤为 Format check → Task 16.1 → Task 9.7/9.8 → Clippy → Test → Rust static gates，保证 `Test` 是该 job 最后一次全量编译；把 `purity` section 从 `Rust (Linux)` 迁到 `governance-static`（`ECHO_PURITY_REQUIRE_BUILD=1`，位于 coverage 之前）。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；`node scripts/verify/checks/task-16.1.mjs`、`task-9.1.mjs` 与 `task-9.7`/`task-9.8` 相关检查退出 0。

## 2. 依赖审计分层、删除 dependency-audit job、新增定时 workflow

- [ ] 2.1 删除 `ci.yml` 的 `dependency-audit` job，把 `pnpm verify:task -- 1.7` 并入 `governance-static`；在线 RUSTSEC 公告扫描按事件分层：PR 只跑离线半，`main` 跑在线半，PR 跳过在线半时打印显式 note。验证：`node scripts/verify/checks/task-1.7.mjs` 退出 0；`rg -n 'dependency-audit' .github/workflows/ci.yml` 无输出。
- [ ] 2.2 新增 `.github/workflows/dependency-audit.yml`，仅含 `schedule` 与 `workflow_dispatch` 触发，1 个 job 真实执行在线扫描。验证：`rg -n 'schedule|workflow_dispatch|task-1.7' .github/workflows/dependency-audit.yml` 命中三项；结构门禁断言该 workflow 存在。

## 3. 缓存治理（shared-key + save-if）

- [ ] 3.1 删除现存 14 条 Actions 缓存（`gh api -X DELETE`），避免旧配额继续触发淘汰。验证：`gh api repos/zsxink/Echo/actions/cache/usage` 的 `active_caches_count` 归零或仅剩本次实施后的新缓存。
- [ ] 3.2 为所有 `Swatinem/rust-cache` 步骤统一 `shared-key`（矩阵 job 用 `${{ matrix.cache }}`，`governance-static` 用 `governance-static`），写者恰好 6 个（`linux-workspace`、`windows-core`、`windows-desktop`、`macos-core`、`macos-desktop`、`governance-static`）且 `save-if: ${{ github.ref == 'refs/heads/main' }}`，其余 job `save-if: false`。验证：`node scripts/verify/checks/task-1.5.mjs` 的缓存写者断言退出 0；`rg -n 'save-if' .github/workflows/ci.yml` 的 true 条件只出现在 6 个 writer job。

## 4. task-gates 4 → 3 分片

- [ ] 4.1 把 `task-gates` 的 `frontend-a` 与 `frontend-b` 合并为 `frontend`，保留 `core` 与 `desktop`；三片并集仍覆盖全部 42 项 task 且无重复。验证：`node scripts/verify/checks/task-1.5.mjs` 的分片并集断言退出 0；`rg -n 'frontend-a|frontend-b' .github/workflows/ci.yml scripts/verify` 无输出（新分片计划文件除外）。

## 5. 注入证明 2 → 3 分片

- [ ] 5.1 把注入证明拆为 3 片，按实测成本显式标注 `shard:`；嵌入前端条目固定在构建 shell 的分片（shard 1）；分片计划放入新文件以避开 `injection-suite.mjs` 的 1000 行上限；同步 `injection-governance.mjs` 与 `task-1.5.mjs` 的分片集合为 `[1, 2, 3]`。验证：`node scripts/verify/injection-suite.mjs --shard=1/3`、`--shard=2/3`、`--shard=3/3` 各自退出 0；`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [ ] 5.2 断言分片计划无空片、无重叠：计划键集等于注入套件 id 集。验证：`node scripts/verify/injection-suite.mjs --shard=1/3` 等三片输出无空片；`node scripts/verify/check-scale.mjs` 退出 0。

## 6. 场景命令与任务脚本的 cargo 形态收敛

- [ ] 6.1 在 `run-scenario.mjs` 的 runner 层（`groupedTestCommand`）把无额外参数的按包场景 cargo 命令归一化为 `cargo test --workspace --all-targets --all-features`；注册表保留按包原始命令，`validate-scenario-commands` 语义不变。验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0；`node scripts/verify/run-scenario.mjs -- --automated` 场景与证据数不变（561/561）。
- [ ] 6.2 给 `task-9.1.mjs` 的 `cargo check` / `cargo test` 与 `task-16.1.mjs` 的 `cargo check` 补 `--all-features`（`task-9.1` 的 `--lib` 保留）；把 `task-12.4.mjs` 的三次独立 vitest 合并为一次多文件调用；注入证明 shell 构建改为 `cargo build -p echo-app --all-features`。验证：`node scripts/verify/checks/task-9.1.mjs`、`task-16.1.mjs`、`task-12.4.mjs` 退出 0；`node scripts/verify/injection-suite.mjs --shard=1/3` 退出 0。

## 7. 重写 ci-structure 注入证明

- [ ] 7.1 在 `injection-governance.mjs` 按新结构重写 ci-structure 注入证明：删除 matrix 分片、把 clippy 移到 `Test` 之后、把 `purity` 从 `governance-static` 删除、把 injection 分片数改回 2，四类注入都必须让 `task-1.5` 失败。验证：`node scripts/verify/injection-governance.mjs` 退出 0，且四类注入各自产生预期的失败信息。
- [ ] 7.2 同步 `task-1.5.mjs` 的 `assertAlwaysRuns` 覆盖新 job id 与 `purity` section 映射。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；`rg -n 'purity|alwaysRuns' scripts/verify/checks/task-1.5.mjs` 命中新映射。

## 8. 本地全量验证与耗时测量

- [ ] 8.1 本地按分片各跑一遍测试，确认无隐式顺序依赖；在 `cargo test --workspace --all-targets --all-features` 之后运行两个 validator，合计 < 30s。验证：各分片命令退出 0；validator 耗时记录 < 30s。
- [ ] 8.2 **独占**运行 `pnpm verify:governance`（不与 cargo、node 扫描或 rg 并行，避免 5 秒墙钟断言被干扰）。验证：整门退出 0，注入自证全部通过；失败只修本 change 引入的问题，既有 flake 如实记录。
- [ ] 8.3 记录 PR run 的总墙钟与每个 job 墙钟，确认 < 10 分钟；合并后确认 `main` 的 5 个 rust 分片全部执行（`desktop` 不得 skipped）、连续两次 run 的缓存主键命中且 Test 编译数 < 50 个 crate、缓存总量 < 6 GB。验证：`gh run view` 与 `gh api repos/zsxink/Echo/actions/cache/usage` 的结果写入 PR 正文。
