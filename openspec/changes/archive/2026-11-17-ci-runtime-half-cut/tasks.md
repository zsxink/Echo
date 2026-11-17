# Tasks

## 1. 场景委托与 manifest

- [x] 1.1 在 `scripts/verify/scenario-commands.mjs` 的注册表层为 `CORE_ALL` / `DESK_ALL` / `APP_ALL` 条目补 `delegatedTo: "workspace-test"`，为 `VITEST_ALL` 条目补 `delegatedTo: "frontend-test"`；运行 `node scripts/verify/gen-scenario-manifests.mjs --write`，并验证 `node scripts/verify/gen-scenario-manifests.mjs` 无 drift。
- [x] 1.2 在 `scripts/verify/check-scenario-evidence.mjs` 的 `DELEGATED_GATES` 加入两个新 gate，并把 `CI_GATES` 从「`pnpm verify:<gate>`」改为每个 gate 的真实命令匹配（`cargo test --workspace --all-targets --all-features`、`pnpm --filter @echo/desktop test -- --run`）；验证 `node scripts/verify/check-scenario-evidence.mjs` 通过。
- [x] 1.3 在 `scripts/verify/run-scenario.mjs` 保持「只按显式 `delegatedTo` 跳过」的语义，新增一条注释说明两个新 gate；验证 `node scripts/verify/run-scenario.mjs SFI-R12-S01` 仍实际执行，`node scripts/verify/run-scenario.mjs -- --automated --delegated=workspace-test,frontend-test` 只跳过声明了对应 gate 的场景。

## 2. task 归档与 16.1 静态化

- [x] 2.1 在 `scripts/verify/manifest.json` 把 `1.8 3.1 4.1 5.1 6.1 11.5`（core）、`7.1 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 10.6 11.1 11.2 11.4 16.2`（desktop）、`10.1 10.2 10.3 10.4 10.5 11.3 11.6 12.1 12.2 12.3`（frontend）标记 `archived: true`，理由写明承载 gate（workspace test / frontend test）；验证 `node scripts/verify/run-task.mjs --all` 输出跳过清单且不执行这些 id。
- [x] 2.2 给 `scripts/verify/checks/task-16.1.mjs` 增加 `--static-only`：跳过 `cargo check` / `cargo test`，保留静态契约断言；把 manifest 中 `16.1` 的命令改为 `node scripts/verify/checks/task-16.1.mjs --static-only`；验证 `node scripts/verify/run-task.mjs -- 16.1` 在数秒内通过。
- [x] 2.3 从 `scripts/verify/checks/task-12.5.mjs` 删除与 Frontend job 重复的 `typecheck` 与 `SongList.test.tsx` 调用，保留 50k benchmark 与 artifact 写入；验证 `node scripts/verify/run-task.mjs -- 12.5` 通过并生成 `apps/desktop/benchmark-50k.json`。

## 3. ci.yml 结构

- [x] 3.1 `rust` 矩阵收敛为 Linux `workspace` 单条；删除 `Task 16.1` 与 `Task 9.7 9.8` 步骤；保留 `Format check`、`Clippy`、`Test`、`Rust static gates` 的顺序；验证 `node scripts/verify/checks/task-1.5.mjs` 在更新后的断言下通过。
- [x] 3.2 `rust-desktop` 的 macOS/Windows 分片改为 `packages: "--workspace"`、`clippy: true`，增加 `Format check` 与 `Clippy` 步骤（在 `Test` 之前），保持 job-level `if: github.event_name != 'pull_request'`；验证缓存写者集合为 `linux-workspace`、`macos-desktop`、`windows-desktop`、`governance-static`。
- [x] 3.3 `task-gates` 矩阵收敛为 `core` 单分片，任务列表改为 `1.1 1.3 12.5 13.3`；保留 `Run performance scenarios` 与 `save-if: false`；验证 `pnpm verify:perf` 仍只在 `task-gates` 执行。
- [x] 3.4 `frontend` job 增加 `Task 13.1 (headless E2E)` 步骤（`CHROME_PATH=/usr/bin/google-chrome`、`CHROME_NO_SANDBOX=1`，命令 `node scripts/verify/checks/task-13.1.mjs --no-build`，因为前一步 `Build` 已产出同一份 dist），并把 `Test` 步骤改为 `pnpm --filter @echo/desktop test -- --run --reporter=verbose`；验证 `task-1.5` 断言该步骤、命令与 `Build` 的先后顺序。
- [x] 3.5 `governance-static` 把 `Run governance static gates` 改为 `--sections=registry,static`，新增 main-only `Run coverage gate`（`--sections=coverage`）与 main-only `Install coverage tooling`；增加 `pnpm verify:task -- 16.1` 步骤；验证 `task-1.5` 断言 coverage 的 main-only 条件。
- [x] 3.6 `macos-platform-gate` 在 `Build and verify macOS platform Gate` 之后增加 `Verify embedded frontend freshness`，运行 `node scripts/verify/checks/check-embedded-frontend.mjs --bin target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents/MacOS/Echo`；验证 `task-1.5` 断言该步骤。
- [x] 3.7 删除 `injection-proofs` job；`release-gate` 的命令改为 `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf,workspace-test,frontend-test`；验证 `task-1.5` 断言新命令且不再引用 injection job。
- [x] 3.8 给 `ci.yml` 的 `push` 与 `pull_request` 增加 `paths-ignore` 白名单（`docs/arch-standards-remaining.md`、`docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md`、`docs/interface-terminology.md`），并在 `task-1.5` 中固定该清单；验证新增条目（例如 `README.md`）会让 `task-1.5` 失败。

## 4. 治理门禁与 spec

- [x] 4.1 重写 `scripts/verify/checks/task-1.5.mjs`：更新 `RUST_MATRIX` / `RUST_DESKTOP_MATRIX`、`CACHE_WRITERS`、`EXPECTED_SHARDS`、step 顺序、`SECTION_SELECTOR_STEP` / `SECTION_DEDICATED_STEP`、`assertAlwaysRuns`（移除 `injection-proofs`）、重复执行锁与 `paths-ignore` 常量；验证 `node scripts/verify/checks/task-1.5.mjs` 通过。
- [x] 4.2 从 `scripts/verify/ci-governance.mjs` 移除 `injection` section 与对应 check（proof 套件改为独立命令），保留 `embedded` / `coverage` section；验证 `node scripts/verify/ci-governance.mjs --list-sections` 输出不含 `injection`，且 `task-1.5` 的 `EXPECTED_SECTIONS` 与之同步。
- [x] 4.3 更新 `scripts/verify/injection-governance.mjs`：删除引用 `injection-proofs` shard、旧 task 分片的 proof；重写 `governance-static` sections、coverage、shard packages、step order、perf 分层的注入点与期望；新增 `rust-desktop` clippy 缺失、coverage main-only、embedded 步骤缺失、16.1 静态步骤缺失、paths-ignore 漂移、注入套件重回 job 的 proof；验证 `node scripts/verify/injection-suite.mjs` 通过。
- [x] 4.4 同步 `scripts/verify/injection-shard-plan.mjs` 的条目集合与分片（删除已删除 proof，新增 proof 归入 shard 1）；验证 `node scripts/verify/injection-suite.mjs --shard=1/3`、`2/3`、`3/3` 均通过。
- [x] 4.5 运行 `openspec validate ci-runtime-half-cut --strict`，确认 delta spec 与 artifacts 合法。

## 5. 集成验证与证据

- [x] 5.1 运行 `node scripts/verify/checks/task-1.5.mjs`、`node scripts/verify/check-scale.mjs`、`node scripts/verify/check-scenario-evidence.mjs`，记录退出码与耗时。
- [x] 5.2 独占运行 `pnpm verify:governance`，记录通过输出；再运行 `node scripts/verify/injection-suite.mjs`，记录 proof 数量。
- [x] 5.3 运行 `node scripts/verify/checks/task-13.9.mjs --skip-static --delegated=governance,perf,workspace-test,frontend-test`，确认自动化场景数与委托数符合预期，且不再执行完整 workspace test / vitest。
- [x] 5.4 更新 `artifacts/` 下的验证报告与 PR #157 说明：记录基线 7233s / 17m13s、预计新基线、实际删除项与保留项；验证 `git status` 只包含本次 change 的预期文件。
