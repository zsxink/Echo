# Tasks

> 关联 Issue #158；落地在 PR #157 的分支 `feat/ci-runtime-under-10-min` 上。

## 1. `ci.yml` 删除重复执行

- [x] 1.1 `injection-proofs` 的前端构建只在 shard 1、2 执行：给 `Build the frontend bundle the desktop shell embeds` 步骤加 `if: matrix.shard != 3`，并把下方「no other shard needs either」的过时注释改为准确表述（shard 1 需要 `dist` 做 embedded-frontend 比对，shard 2 的 `task-9.1` 缺 `dist` 会 FAIL，shard 3 两者都不需要）。验证：`rg -n "Build the frontend bundle" -A1 .github/workflows/ci.yml` 显示该步骤带 `if: matrix.shard != 3`；`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [x] 1.2 删除 `rust-desktop` 的 `Format check` 步骤（`cargo fmt --all -- --check`）：`rust` job 的 macOS / Windows `core` 分片已在同一平台、同一 checkout 上执行同一条命令。验证：`rg -n "Format check" .github/workflows/ci.yml` 只命中 `rust`（第 60 行附近）与 `frontend`（`pnpm --filter @echo/desktop format:check`）两处；`node scripts/verify/checks/task-1.5.mjs` 退出 0。
- [x] 1.3 `frontend` 矩阵由三平台收敛为 Linux 单腿：`matrix.include` 只保留 `{ platform: Linux, os: ubuntu-latest }`，保留 `name: Frontend (${{ matrix.platform }})`（检查名仍为 `Frontend (Linux)`），五个质量步骤不动。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；`gh pr checks` 上该 job 只出现一条 `Frontend (Linux)`。
- [x] 1.4 删除 `perf` job，把 `pnpm verify:perf` 作为新步骤 `Run performance scenarios` 加到 `task-gates` 的 `core` 分片：`if: matrix.shard == 'core'`，位置在 `Upload 50k benchmark result` 之后（benchmark artifact 不能因 perf 失败而丢失）。`task-gates` 的缓存步骤保持 `save-if: false`，`--release` 产物不写入任何 key。验证：`rg -n "perf:" .github/workflows/ci.yml` 无 job 级命中；`rg -n "pnpm verify:perf" .github/workflows/ci.yml` 命中 `task-gates` 的新步骤；`node scripts/verify/check-scenario-evidence.mjs` 退出 0（`perf` 委托仍有执行位置）。
- [x] 1.5 `task-13.2` 删除裸跑的 `player_smoke`：删除第 48 行的 `run("cargo", ["test", "-p", "echo-desktop", "--all-features", "--test", "player_smoke"])`，并把第 74 行报告文案里的 `player_smoke (real libmpv)` 改为指向 `task-8.12` 的真实 libmpv 运行（`DYLD_LIBRARY_PATH` + 命中 skip 即 FAIL）。验证：`rg -n "player_smoke" scripts/verify/checks/task-13.2.mjs` 只命中说明性文字且不含 `--test` 调用；`node scripts/verify/checks/task-1.5.mjs` 退出 0。**本地不运行 task-13.2**（它会覆写用户未提交的 `artifacts/native-e2e-13.2.txt`）。

## 2. 结构门禁同步（`task-1.5.mjs`）

- [x] 2.1 前端矩阵断言从三平台改为一平台：`assertPlatformMatrix(..., { Linux: { os: "ubuntu-latest" } }, "ci.yml frontend")`；`assertPlatformName` 保持 `Frontend (${{ matrix.platform }})` 不变。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；临时把 macOS 条目加回 `ci.yml` 后该检查以非零退出并指出多出的 `macOS` 条目。
- [x] 2.2 `DESKTOP_ORDER` 由 `["Format check", "Test"]` 改为 `["Test"]`。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；临时把 `Format check` 加回 `rust-desktop` 后，`assertAlwaysRuns` 之外的重复执行断言（2.4）以非零退出。
- [x] 2.3 重写 perf 断言：删除 `step(workflow, "perf", "Run performance scenarios")` 的 job 级 pin，改为在 `task-gates` 中定位 `Run performance scenarios`，断言 (a) `run === "pnpm verify:perf"`，(b) `if` 包含 `matrix.shard == 'core'`（即每个事件都跑，PR 也跑），(c) `task-gates` 的缓存步骤仍是 `save-if: false`（`--release` 不落缓存）。从 `assertAlwaysRuns` 列表移除 `"perf"`。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；把该步骤的 `if` 改为 `matrix.shard == 'desktop'` 后检查以非零退出。
- [x] 2.4 新增反向锁定断言（对应 delta spec 的「同一检查 MUST NOT 在多个 job 中重复执行」）：
  - (a) 遍历 `ci.yml` 所有 job 的步骤，同一 `run` 文本在**不同 job** 出现时，只有在两者 `runs-on` 平台不同时才合法；`cargo fmt --all -- --check` 在 `rust` 与 `rust-desktop` 同时出现必须失败（两者在 macOS/Windows 上重叠）。
  - (b) 断言 `task-13.2.mjs` 源码不含 `--test", "player_smoke` 形态的调用，且 `task-8.12.mjs` 仍含 `DYLD_LIBRARY_PATH` 与 `skipping real playback smoke`。
  - (c) 断言 `injection-proofs` 的前端构建步骤 `if` 含 `matrix.shard != 3`，且 shard 1 的 shell 构建与 embedded-frontend 步骤仍为 `matrix.shard == 1`。
  验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；三条断言各自的注入证明（3.2）全绿。

## 3. 注入证明重写与新增

- [x] 3.1 重写 `ci-structure/perf-job-gated-off-pr`：原 needle `"  perf:\n    name: Performance scenarios\n"` 已不存在。改为两条证明：(a) `ci-structure/perf-step-dropped`——删除 `task-gates` 的 perf 步骤，期望 `FAIL 1.5: ci.yml job 'task-gates' is missing step 'Run performance scenarios'`；(b) `ci-structure/perf-step-narrowed`——把该步骤的 `if` 改成 `matrix.shard == 'desktop'`，期望 perf 步骤必须覆盖 core 分片的失败信息。
- [x] 3.2 新增删除项的反向证明：`ci-structure/desktop-format-check-readded`（把 `Format check` 加回 `rust-desktop`）、`ci-structure/frontend-matrix-widened`（把 macOS 条目加回 frontend 矩阵）、`ci-structure/injection-shard3-frontend-build`（去掉前端构建步骤的 `if: matrix.shard != 3`）、`ci-structure/duplicate-player-smoke`（把裸跑 `player_smoke` 加回 `task-13.2.mjs`）。每条都必须让 `task-1.5` 以非零退出并给出对应的失败信息。
- [x] 3.3 把新增/改写的证明 id 登记进 `injection-shard-plan.mjs` 的 shard 1，并把分片计数从 44/7/5 更新为新值。验证：`node scripts/verify/injection-suite.mjs --shard=1/3`、`--shard=2/3`、`--shard=3/3` 三片各自退出 0；`node scripts/verify/check-scale.mjs` 退出 0。

## 4. 文档同步

- [x] 4.1 更新 `docs/arch-standards-remaining.md` §11 的 CI 拓扑表：`Perf` 行并入 `Task gates (core)`，`Frontend` 行标注单平台，`Injection proof` 行注明前端构建只在 shard 1、2 执行；同步 `docs/native-attestation-playbook.md` 中「独立 perf job」的表述。验证：`rg -n "Perf|Performance scenarios" docs/arch-standards-remaining.md docs/native-attestation-playbook.md` 无「独立 job」表述。

## 5. 本地全量验证

- [x] 5.1 运行 `node scripts/verify/checks/task-1.5.mjs`、`node scripts/verify/check-scale.mjs`、`node scripts/verify/check-scenario-evidence.mjs`、`node scripts/verify/validate-scenario-commands.mjs`，全部退出 0。
- [x] 5.2 **独占**运行 `pnpm verify:governance`（不与 cargo、node 扫描或 rg 并行；5 秒墙钟断言会被任何并发负载干扰）。验证：整门退出 0。失败只修本 change 引入的问题，既有 flake 如实记录。
- [x] 5.3 同步 delta spec 到 `openspec/specs/engineering-governance/spec.md`，归档 change 到 `openspec/changes/archive/2026-11-17-trim-duplicate-ci-executions/`。验证：`npx openspec validate trim-duplicate-ci-executions --strict` 在归档前退出 0；归档后 `npx openspec list` 不再列出该 change。

## 6. 提交与 PR

- [ ] 6.1 以显式路径提交（**禁止** `git add -A` / `git add .`；用户未提交的 `artifacts/fault-injection-13.3.md` 与 `artifacts/native-e2e-13.2.txt` 不得进入提交），推送到 `feat/ci-runtime-under-10-min`。验证：`git show --stat HEAD` 不含 `artifacts/` 下这两个文件；`git status --porcelain` 仍显示它们为 ` M`。
- [ ] 6.2 更新 PR #157 正文：引用 Issue #158，写明四项删除**降低配额与失败面但不缩短关键路径**（热态 Release gate 仍约 11m24s），并把 `task-9.1`（82s）+ `task-13.8`（101s）列为下一个约 180s 的杠杆。验证：`gh pr view 157 --json body` 含 `#158` 与「关键路径」段落。
