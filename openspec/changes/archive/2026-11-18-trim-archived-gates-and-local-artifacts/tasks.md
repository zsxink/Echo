# Tasks

## 1. 引用检查先行：证明删除集合与保留例外

- [x] 1.1 新增 `scripts/verify/check-archived-gates.mjs`：按 design D1 的执行入口集合（`.github/workflows/ci.yml`、`run-task.mjs`、`injection-governance.mjs`、`ci-governance.mjs`、`self-test.mjs`、`validate-scenario-commands.mjs`、`check-scenario-evidence.mjs`、`package.json`、`scripts/echo.mjs`）扫描门禁脚本引用，明确排除 `manifest.json` 自身、`checks/` 内部与历史文档，输出"archived-only 且无执行引用"的脚本集合。验证：删除前运行 `node scripts/verify/check-archived-gates.mjs` 退出 0，输出恰好 41 个可删脚本与 1 个例外 `task-13.2.mjs`，且 41 个路径与 proposal/design 记录的集合逐项一致。
- [x] 1.2 为引用检查补存在性与悬空路径断言：`task-13.2.mjs` 必须存在且 `injection-governance.mjs` 仍指向它；`manifest.json` 与 `ci.yml` 不得引用已移除脚本。验证：运行 `node scripts/verify/check-archived-gates.mjs` 退出 0；临时把 guard 目标改名、临时在 `manifest.json` 写入一个不存在的 `task-*.mjs` 路径，两次都必须非零退出并指出具体项，恢复后再次退出 0。

## 2. 删除归档门禁脚本并同步治理清单

- [x] 2.1 删除 41 个经 1.1 证明无执行引用的 `scripts/verify/checks/task-*.mjs` 文件；`task-13.2.mjs` 原样保留。验证：`git status --short | grep '^ D' | grep -c 'scripts/verify/checks/task-.*\.mjs'` 为 41；`node scripts/verify/check-archived-gates.mjs` 退出 0；`test -f scripts/verify/checks/task-13.2.mjs` 成立。
- [x] 2.2 按 design D3 同步 `scripts/verify/manifest.json`：41 个 task 保留 `id` / `archived: true` / `archivedReason`，移除指向已删脚本的 `commands`；`task-13.2` 条目保留 `commands`，并在 `archivedReason` 中记录 `injection-governance.mjs` 为消费者与保留理由。验证：`node scripts/verify/check-archived-gates.mjs` 退出 0 且报告无悬空路径；`node scripts/verify/run-task.mjs --all` 打印的 archived 跳过数量与实施前一致（47），输出中无已删脚本路径；显式调用一个已清理 id（如 `node scripts/verify/run-task.mjs -- 1.4`）报 `no verification command registered`，调用 `13.2` 仍可执行。
- [x] 2.3 同步 `.github/workflows/ci.yml` 中关于"旧 42-id 清单 / archived 门禁"的注释，使描述与实际执行集合一致；不改动任何 job / step / 矩阵字段。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；对比实施前后 `verify:task` 显式调用的 id 集合一致。

## 3. 本地临时产物清理（仅限被忽略文件）

- [x] 3.1 删除 `apps/desktop` 下被 `.gitignore` 覆盖的临时产物：`dist/`、`node_modules/.vite-temp/`、`node_modules/.tmp/`、`vitest.config.ts.timestamp-*.mjs`、`benchmark-50k.json`。验证：`git status --short` 无被跟踪文件变更；`git ls-files --others --ignored --exclude-standard apps/desktop | grep -E '(vitest\.config\.ts\.timestamp-|benchmark-50k\.json|/dist/)'` 无输出。注意：`dist/` 是 Tauri `generate_context!()` 的前置构建产物，删除后如需编译或 `cargo check`，须先 `pnpm -C apps/desktop build` 重建；重建产物仍属被忽略文件，不作为交付资产。
- [x] 3.2 删除 `artifacts/` 下 42 个未跟踪草稿（一次性重构脚本、场景快照、日志与临时报告），保留 2 个被 git 跟踪的验收资产。验证：`git ls-files artifacts` 仍输出 `artifacts/fault-injection-13.3.md` 与 `artifacts/native-e2e-13.2.txt`；`find artifacts -type f | wc -l` 为 2。

## 4. 治理验证与交付证据

- [x] 4.1 独占运行与本次改动直接相关的治理验证：`node scripts/verify/check-archived-gates.mjs`、`node scripts/verify/checks/task-1.3.mjs`（verify runner / self-test）、`node scripts/verify/checks/task-1.5.mjs`（manifest archived 条目与 CI 结构）、`pnpm verify:self-test`、`pnpm verify:governance`；再运行 `cargo check --workspace --all-targets` 确认零警告。不运行 `pnpm verify:task --all`：本地实测 1800s 内仅完成 69/93 个 active task，且 task 1.9 因 aarch64 macOS bundle 冷启动环境限制失败（单独复现、文件未被本 change 触及），全套在 CI 按分片执行。验证：全部命令退出 0；`cargo check` 的 warning/error 行数为 0；记录各命令输出作为 PR 证据。前置：若 3.1 刚清理过 `dist/`，先 `pnpm -C apps/desktop build` 重建，否则 `generate_context!()` 会因缺少前端产物而编译失败（已实测）。
- [x] 4.2 核对实际执行集合未变：对比实施前后 `pnpm verify:task --all` 的执行 id 集合与 `ci.yml` 的 task-gates 分片及显式调用项，确认删除只影响 archived 条目。验证：`node scripts/verify/checks/task-1.5.mjs` 退出 0；PR 正文记录执行集合前后一致的对照结果。
- [x] 4.3 按项目流程收尾：确认任务全部勾选、验证记录齐全后，执行 delta spec 同步与 `openspec validate --strict`，归档本 change，再创建关联 Issue #160 的 Pull Request。验证：`openspec validate --strict` 退出 0；`openspec status --change trim-archived-gates-and-local-artifacts` 显示已归档；PR 正文包含 1.1 保存的 41 项清单与 `task-13.2` 例外说明。
