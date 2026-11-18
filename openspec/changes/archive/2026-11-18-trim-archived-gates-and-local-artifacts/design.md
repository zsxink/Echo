# Design

## Context

- 动机与范围见 `proposal.md`；行为要求见 `specs/engineering-governance/spec.md` 的新增 requirement。
- 当前治理引用关系分散在四处：`scripts/verify/manifest.json`（登记 task 与 `commands`）、`scripts/verify/run-task.mjs`（按 id 执行/link 跳过 archived）、`.github/workflows/ci.yml`（执行入口与注释）、`scripts/verify/injection-governance.mjs`（注入证明的 guard 目标）。此外 `openspec/changes/archive/` 与 `docs/` 会文字提及全部 task 名。
- 截至当前 HEAD 的可复跑事实：manifest 共 140 个 task，其中 47 个 `archived: true`；仅被 archived 条目引用的脚本 42 个；排除仍被注入治理 guard 引用的 `task-13.2.mjs` 后，实际可删 41 个（2,381 行 / 96 KB）。
- 本 change 只改动治理脚本与本地被忽略文件，不触碰生产代码、Core/IPC/DTO、数据库或同步载荷。分层与依赖方向保持现状：CI 调用 `scripts/verify/**`，治理脚本读取仓库文本；不新增跨层依赖，不引入新的抽象或设计模式。
- 适用规范：`openspec/CODE_STANDARDS.md` §2.1（文件规模）、§8（交付门槛）、§9.1（规格复核与残留清理门禁）。`ci.yml` 的 job / step / 矩阵由 `task-1.5.mjs` 结构门禁锁定，本 change 不改其结构。

## Goals / Non-Goals

**Goals:**

- 以可复跑的证据确定"哪些门禁脚本已无执行引用"，据此删除 41 个脚本，并保持 1 个有消费者的例外。
- 让 manifest、CI 注释与 `run-task.mjs` 的 archived 处理一致，消除指向已删除脚本的悬空路径。
- 清理本地被忽略的临时产物，且不触碰版本控制跟踪的验收资产。

**Non-Goals:**

- 不改变持续集成实际执行的检查集合、平台覆盖与断言强度；不重写 `task-1.5.mjs` 的结构断言或注入证明。
- 不修改任何产品代码、接口契约、数据模型或构建产物内容。
- 不做中风险项（vendored DLL 外置、归档文档瘦身、task 清单整体收敛）。

## Decisions

### D1. 以"实际执行引用"而非"全仓文字提及"作为删除判据

- 执行入口集合限定为：`ci.yml`、`run-task.mjs`、`injection-governance.mjs`、`ci-governance.mjs`、`self-test.mjs`、`validate-scenario-commands.mjs`、`check-scenario-evidence.mjs`、`package.json`、`scripts/echo.mjs`。
- 明确排除：`manifest.json` 自身（它只是登记，不构成执行）、`scripts/verify/checks/` 内部互相引用、历史 OpenSpec 文档与 `docs/`（只作文字记录）。
- 理由：归档文档会逐一列出全部 task 名，按文件名全仓匹配会把 41 个待删脚本全部误判为"仍有引用"；按执行引用判定才能得到可执行的删除集合。
- 替代方案：逐脚本人工审阅调用关系。否决——不可复跑，无法在 PR 中自证。

### D2. `task-13.2.mjs` 作为有消费者的例外登记

- 该脚本属于 archived 集合，但 `scripts/verify/injection-governance.mjs:230-236` 把它作为防回归 guard 目标；删除会让注入治理套件失效。
- 处理：脚本保留；在 manifest 对应条目的 `archivedReason` 中记录消费者与保留理由；引用检查 MUST 同时断言其存在性与 guard 仍指向它。
- 理由：归档只说明"其断言已被别的 gate 承载"，不说明"脚本对象可以删除"；消费者证明是删除的前置条件。

### D3. manifest 采用"保留条目、移除 commands"而非整条删除

- 对 41 个待清理 task，保留其 `id` / `archived: true` / `archivedReason`，移除 `commands` 中指向已删脚本的路径；`task-13.2` 例外保留其 `commands` 与 guard 说明。
- 理由：整条删除会改变 `run-task --all` 的 skip 计数与输出，并可能波及 `task-1.5.mjs` 对已登记 task 并集的断言；保留条目可维持治理清单的覆盖面叙述，同时消除悬空路径。
- 兼容影响：`run-task --all` 仍打印 47 个跳过项，与实施前一致。实施中发现既有注释「an explicit id still runs」只适用于仍保留脚本的 archived 条目：显式调用已清理的 41 个 id 时，`run-task.mjs` 报 `no verification command registered` 而非运行脚本。这是删除脚本的必然结果，不是额外检查损失；`ci.yml` 注释已同步修正，13.2 例外与共享脚本的 archived 条目仍可显式运行。

### D4. 本地清理限定在"被忽略文件 + 显式路径白名单"

- 仅处理 `apps/desktop/dist/`、`node_modules/.vite-temp/`、`node_modules/.tmp/`、`vitest.config.ts.timestamp-*.mjs`、`benchmark-50k.json` 与 `artifacts/` 下未跟踪草稿。
- 以 `git ls-files --others --ignored --exclude-standard` 与 `git ls-files` 交叉核对：只删被忽略且不在白名单外的文件；`artifacts/fault-injection-13.3.md` 与 `artifacts/native-e2e-13.2.txt` 被 git 跟踪，必须保留。
- 理由：避免用宽泛的清理命令误删开发者本机未提交的文件。

### D5. 引用检查作为独立可复跑入口，不接入 CI 结构门禁

- 新增独立检查脚本 `scripts/verify/check-archived-gates.mjs`，提供 `node scripts/verify/check-archived-gates.mjs` 入口，断言三件事：待删集合与 D1 的执行引用计算结果一致；`task-13.2.mjs` 存在且 guard 仍指向它；manifest 与 `ci.yml` 无指向已删脚本的悬空路径。
- 本 change 中不把它注册为 `ci-governance` 的新 section，也不改 `ci.yml`：接入新 section 需要同步 `task-1.5.mjs` 的 section 映射与注入证明，扩大变更面并触碰被锁定的结构。
- 理由：本 change 的核心是删除残留，检查脚本的职责是让删除可自证；长期强制接入可作为后续独立 change。代价是该检查不会在每次 PR 自动运行，需由本 change 的验证步骤与代码审查保证。

### D6. 实施顺序：先证明、再删除、最后清理

1. 先落地 D5 的检查并在删除前运行，确认 41 个待删脚本确无执行引用、1 个例外确有消费者。
2. 删除 41 个脚本并执行 D3 的 manifest 同步与 CI 注释同步。
3. 再执行 D4 的本地清理（本地文件不影响 CI）。
4. 最后运行治理验证：`verify:self-test`、`verify:governance`、`verify:task --all`，以及引用检查与 `cargo check`。
- 理由：先证明后删除可以在误判时立刻停下，避免"删完才发现有消费者"。

## Risks / Trade-offs

- **本地全量 `verify:task --all` 不可行** → 实测 1800s 内仅完成 69/93 个 active task，且 task 1.9 因 aarch64 macOS bundle 冷启动环境限制失败（单独复现、未被本 change 触及）；验证改为直接相关的 task-1.3 / task-1.5 + 治理全套，全套由 CI 分片承载。这不是降低门禁强度：被删脚本全部来自 archived 集合，不影响任何 active 检查。

- **历史文档提及被删脚本名** → 明确文字提及不构成执行引用；历史记录可从 git 追溯，不修改归档文档。
- **`task-13.2.mjs` 被误删导致注入治理失效** → D2 的消费者登记与 D5 的存在性断言双重锁定；验证阶段必须跑 `verify:governance`。
- **manifest 移除 `commands` 后显式调用 archived id 的行为变化** → 保持 archived 语义与 skip 输出不变，在 tasks 的验证中记录实际输出差异。
- **新引用检查未接入 CI，长期回退无自动拦截** → 作为本 change 的强制验证命令写入 tasks，并在 PR 说明其定位；后续如需强制可单独 change 接入 section。
- **本地清理误删用户文件** → D4 的白名单与 git 跟踪状态交叉核对；只删被忽略文件。
- **清理 `dist/` 与编译前置冲突** → 实施中发现 `apps/desktop/dist` 是 Tauri `generate_context!()` 的必需输入，删除后 `cargo check --workspace --all-targets` 以 `frontendDist ... does not exist` 失败；在验证阶段先 `pnpm -C apps/desktop build` 重建即可恢复，重建产物仍被 `.gitignore` 覆盖，不算交付资产。这是本地产物的重建，不是范围扩大。
- **41 个脚本中个别被未在 D1 集合内的入口引用** → 删除前运行引用检查，若出现未登记消费者则立即停止并回到 D1 扩充入口集合。

## Migration Plan

- 在独立分支实施，关联 Issue #160；按 D6 顺序执行，删除前保存引用检查输出作为 PR 证据。
- 无数据迁移、无 schema 变化、无构建产物变化；回滚使用 `git revert`，被删脚本从 git 历史恢复。
- 实施完成并通过验证后，按项目流程同步 delta spec、归档 change，再创建关联 issue #160 的 Pull Request。

## Open Questions

无。是否把引用检查接入 CI section 会改变本 change 的结构门禁范围，属于后续独立 change 的决策，不在本 change 内悬置。
