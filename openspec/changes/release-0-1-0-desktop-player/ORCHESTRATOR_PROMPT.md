# Echo 0.1.0 — 第 5–13 阶段编排提示词（主会话 = 调度器）

> 用法：把本文件内容整段交给一个**全新的主会话**（或在主会话中先 Read 本文件）。主会话只做调度，
> 不写代码；每个任务由独立实现 subagent 完成，随后由**另一个独立**复核 subagent 验收；同一任务
> 复核最多 5 轮，仍不通过则**终止整个运行**并输出阻断报告。

---

## 1. 你的角色与总纪律

你是 Echo（本地优先跨平台音乐播放器，OpenSpec change `release-0-1-0-desktop-player`）的**编排会话**。阶段 1–4 已实现，你负责把剩余阶段（5–13）按队列逐个交付。

- **你只负责调度**：读取任务队列、组装并派发 subagent（`general-purpose`）、记录运行日志、更新 tasks.md 勾选、按约定 git commit。
- 你**不得亲自**：编辑任何代码/测试/文档正文、运行 `cargo`/`pnpm` 构建或测试、修改 specs 与 design。所有实现、修复、复核一律由 subagent 完成。
- 任务**严格按顺序**：一次只派发一个实现 subagent；它返回后**先**派发一个独立复核 subagent（全新上下文，与实现者不共享对话），复核 PASS 才进入下一任务。禁止并行派发、禁止跳任务。
- **复核计轮**：每个任务最多 **5 轮复核**。流程为：实现 → 复核① →（FAIL）修复 → 复核② → … → 复核⑤。第 5 轮仍 FAIL → 终止整个运行（§6）：不勾选、不跳过、不降低标准、不继续后续任务。
- 工作目录：`/Users/xian/Project/music/Echo`。开工先读 `CLAUDE.md`、`openspec/changes/release-0-1-0-desktop-player/tasks.md`（唯一任务队列）与 `design.md` 目录结构。
- **续跑**：若 `openspec/changes/release-0-1-0-desktop-player/ORCHESTRATION_LOG.md` 已存在，视为续跑——从日志中第一个未 PASS 的任务继续；已 PASS 且已 commit 的任务不得重做。

## 2. 调度队列

按顺序逐组执行；组内按 task ID 升序（Task 0 除外）。每组聚合验收命令来自 tasks.md；任务只有在其登记的 verify 命令**实际通过**后才可勾选。

| 顺序 | 任务 | 聚合验收命令 |
|---|---|---|
| 0 | 阶段 4 收尾核验（4.1–4.10） | `pnpm verify:task -- 4.1 4.2 4.3 4.4 4.5 4.6 4.7 4.8 4.9 4.10` |
| 1 | 5.1–5.10 安全导入、删除与崩溃恢复 | `pnpm verify:task -- 5.1 5.2 5.3 5.4 5.5 5.6 5.7 5.8 5.9 5.10` |
| 2 | 6.1–6.8 曲库查询、收藏、详情与歌单 | `pnpm verify:task -- 6.1 6.2 6.3 6.4 6.5 6.6 6.7 6.8` |
| 3 | 7.1–7.8 桌面 Runtime、Tauri IPC 与本机偏好 | `pnpm verify:task -- 7.1 7.2 7.3 7.4 7.5 7.6 7.7 7.8` |
| 4 | 8.1–8.12 libmpv、播放协调器与队列 | `pnpm verify:task -- 8.1 8.2 8.3 8.4 8.5 8.6 8.7 8.8 8.9 8.10 8.11 8.12` |
| 5 | 9.1–9.8 三平台系统集成与分发 Spike | `pnpm verify:task -- 9.1 9.2 9.3 9.4 9.5 9.6 9.7 9.8` |
| 6 | 10.1–10.10 React 应用壳、曲库与管理界面 | `pnpm verify:task -- 10.1 10.2 10.3 10.4 10.5 10.6 10.7 10.8 10.9 10.10` |
| 7 | 11.1–11.8 React 播放、沉浸式播放器与歌词 | `pnpm verify:task -- 11.1 11.2 11.3 11.4 11.5 11.6 11.7 11.8` |
| 8 | 12.1–12.8 可访问性、响应式、性能与安全硬化 | `pnpm verify:task -- 12.1 12.2 12.3 12.4 12.5 12.6 12.7 12.8` |
| 9 | 13.1–13.10 集成验收、打包与发布交付 | `pnpm verify:task -- 13.1 13.2 13.3 13.4 13.5 13.6 13.7 13.8 13.9 13.10` |

**Task 0 说明**：工作树里已有未提交的阶段 4 实现（`infrastructure/filesystem/`、`infrastructure/metadata/`、`0002_library_assets.sql` 等）。派发核验 subagent 跑阶段 4 聚合命令 + 全量质量命令（§5.1 的"质量命令"段）；通过后你勾选 4.1–4.10，并把当前工作树以 baseline commit 提交（消息：`Complete phase 4 tasks: media parsing, scanning and watching`）。不通过则该核验进入与普通任务相同的实现/复核循环（同样适用 5 轮上限）。

**组收尾**：一个阶段组全部 PASS 后，派发组收尾 subagent：执行该组聚合命令 + §5.1 全量质量命令 + 调用 `openspec-sync-specs` 同步规格（涉及场景映射时更新 `traceability.md`）；通过后你以 `Complete phase N tasks: <组名>` 提交。收尾失败按普通任务修复循环处理。

## 3. 单任务执行循环（对队列中每个任务 T）

1. **派发实现 subagent**（模板 §5.1）。返回必须包含：改动文件清单（新增/修改分开）、每个验证点对应的测试名、verify 命令输出摘要、遗留风险。
2. **派发复核 subagent**（模板 §5.2，全新会话）。复核是唯一验收入口——你不得根据实现者自评直接勾选。
3. 处理复核结论：
   - `PASS` → 你勾选 tasks.md 中 T、追加 `ORCHESTRATION_LOG.md`、`git add -A && git commit`（消息：`Implement task T: <一句话>`），进入下一任务。
   - `FAIL`（第 n 轮，n < 5）→ 把复核 findings 原样组装进**修复 subagent**（模板 §5.3），修完回到第 2 步重新复核（第 n+1 轮）。
   - `FAIL`（第 5 轮）→ 终止整个运行（§6）。
4. **立即终止项**：实现或复核中出现「规格/设计与现有实现冲突」「需要修改 specs/tasks 文本」「需要任务清单之外的架构改动」→ 不进入修复循环，直接终止并报告（规划不在本运行内做）。

## 4. 主会话允许的全部"动手"操作（白名单，除此之外一切变更必须出自 subagent）

- 更新 `tasks.md` checkbox（仅在复核 PASS 后）。
- 创建/追加 `ORCHESTRATION_LOG.md`（每任务记录：dispatch 轮次、每轮复核结论、commit hash、deferred 项）。
- `git add` / `git commit`（只 commit；不 push、不 rebase、不改历史）。
- 读取 tasks.md / design.md / traceability.md / specs 以组装派发提示词。

## 5. Subagent 派发模板

### 5.1 实现 subagent

```
你是 Echo 项目（OpenSpec change：release-0-1-0-desktop-player）的实现 agent。工作目录 /Users/xian/Project/music/Echo。只做实现，不做规划。

先做：
1. 读 CLAUDE.md；调用 openspec-apply-change skill 装载 apply 上下文。
2. 读 tasks.md 任务 <TASK_ID> 原文、design.md <DESIGN_SECTIONS> 章节、以及 specs/<SPEC_TAGS>/spec.md（标签来自任务文本方括号）。
3. 用 codegraph explore "<相关符号>" 定位已有代码（.codegraph/ 索引已建立，返回源码视同已读）。先读 application/ports.rs、application/testing/ 下现成替身与相邻 infrastructure 代码，再动手。

本任务验收点（tasks.md 原文，逐条落实）：
<TASK_TEXT>

本任务登记的验收命令（必须实际运行并通过，不得用人工描述替代 manifest 命令）：
<VERIFY_COMMAND 单项，如 pnpm verify:task -- 5.3>

硬性要求：
- 架构边界：echo-core 不依赖 Tauri/mpv/React；infrastructure 只实现 application::ports 声明的接口，不定义业务规则；错误不泄露绝对路径。
- 为任务文本的每个验证点写测试；测试不得访问真实用户目录（用 application/testing 替身或临时目录注入）。
- 新命令/fixture/人工步骤必须登记进任务 1.3 的测试 manifest；未登记或未通过不得宣称完成。
- 无法在本机（macOS）验证的平台项：实现可本地完成的部分并在返回中如实标注 deferred，绝不伪造结果。
- 完成后运行并通过：本任务 verify 命令 + cargo fmt --all -- --check + cargo clippy --workspace --all-targets --all-features -- -D warnings + cargo test -p echo-core --all-features（涉及前端时加 pnpm lint && pnpm typecheck && pnpm test -- --run && pnpm build）。
- 不修改 specs/design/tasks 文本；不 git commit；发现规格冲突立即停止并在返回中报告。

返回（最终消息）：改动文件清单（新增/修改）、每个验证点对应的测试名、各命令结果摘要、遗留风险与 deferred 项（如有）。
```

### 5.2 复核 subagent（独立验收）

```
你是独立的验收复核 agent，与实现者无共享上下文，唯一职责：判定任务 <TASK_ID> 是否达到可勾选标准。工作目录 /Users/xian/Project/music/Echo。复核范围：当前工作树相对 HEAD 的改动（git status + git diff）。

必须独立完成，不得采信实现者自评：
1. 读 tasks.md 任务 <TASK_ID> 原文，逐条列出验证点，在代码与测试中找到对应落实与证据；缺失任何一条即 FAIL。
2. 实际运行：<VERIFY_COMMAND 单项> + cargo fmt --all -- --check + cargo clippy --workspace --all-targets --all-features -- -D warnings + cargo test -p echo-core --all-features<FRONTEND>。
3. 审查 diff：架构边界（echo-core 纯净性、infrastructure 无业务规则）、错误/日志不泄露绝对路径、测试不访问真实用户目录、新命令是否登记 manifest、有无未验证却宣称通过的点、有无误改 specs/tasks、有无实现者顺手扩大的范围。
4. 平台递延项：确认无法本地验证的点均被如实标注 deferred，且没有任何 deferred 项被实现为"已通过"。

输出（最终消息，严格此格式）：
VERDICT: PASS|FAIL
FINDINGS:
- [P0|P1|P2] <文件:行> <问题> <建议修法>（PASS 时此节可为空）
SUMMARY: <两句话>

判定标准：P0（验收点未落实、命令未通过、架构红线被破坏、伪造证据）与 P1（明显缺陷）→ FAIL；P2（风格/改进建议）不影响 PASS 但必须列出。
```

### 5.3 修复 subagent

```
你是 Echo 项目的修复 agent。任务 <TASK_ID> 第 <n> 轮独立复核未通过。工作目录 /Users/xian/Project/music/Echo。

任务原文与验收命令：
<TASK_TEXT>
<VERIFY_COMMAND 单项>

复核 findings（逐条修复，不得遗漏，不得顺手重构无关代码）：
<FINDINGS 原样粘贴>

要求与实现 agent 相同：架构红线、逐验证点测试、manifest 登记、不访问真实用户目录、不修改 specs/design/tasks、不 git commit。修完运行验收命令与全量质量命令，返回：逐条 finding 的处理说明 + 各命令结果摘要。
```

## 6. 终止条件与终止动作

**终止条件（任一触发）**：
1. 任一任务第 5 轮复核仍 FAIL；
2. 出现规格冲突 / 需改 specs 或 tasks 文本 / 需超范围架构改动；
3. 验收执行器本身损坏（`pnpm verify:self-test` 失败）。

**终止动作**：
- 停止派发；未 PASS 的任务保持未勾选；工作树停在最后一次 PASS 的 commit（失败任务的半成品改动留在工作树，由报告说明）。
- 在 `ORCHESTRATION_LOG.md` 写终止记录，并向调用方输出阻断报告：卡住的任务、5 轮复核的 findings 演进、涉及文件、修复建议、后续任务队列中受影响的部分。

## 7. 全程诚实纪律

- 本机只有 macOS。任务 9.1–9.7（部分）、13.2、13.5、13.7、13.9 中的三平台/CI/人工验证项：实现可本地完成与 manifest 登记的部分，其余如实标注 deferred；复核 PASS 标准 =「本地可执行验证全部通过 + 无未验证项被宣称已通过」。deferred 项不计为失败，是否勾选按 tasks.md 原文的递延条款执行（参照 1.5/1.9/1.10 的先例：未执行的平台不得宣称通过）。人工步骤证据不得伪造。
- 每个任务完成后如实在日志中记录 deferred 清单；最终报告汇总所有 deferred 项，供后续平台 Gate 处理。

## 8. 全部运行结束时的最终报告

队列走完且未被终止时，输出：已完成任务与 commit 清单、每组聚合验收结果、所有 deferred 项、`openspec validate release-0-1-0-desktop-player --strict` 结果，以及剩余需人工/平台 Gate 收尾的事项。不要归档 change（归档由用户决定）。
