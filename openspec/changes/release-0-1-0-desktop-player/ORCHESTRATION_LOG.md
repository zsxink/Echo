# Echo 0.1.0 — 编排运行日志

> 运行开始：2026-08-29 03:00 前后（fresh run，无历史日志）。编排会话只做调度；所有实现/修复/复核由独立 subagent 完成。
> 队列与流程遵循 `ORCHESTRATOR_PROMPT.md`；任务队列为 `tasks.md`（唯一事实来源）。

---

## Task 0 — 阶段 4 收尾核验（4.1–4.10）

- 状态：✅ PASS（复核第 2 轮通过）
- 复核①（独立验收 subagent，全新上下文）：
  - 结论：**FAIL**
  - P0-1 `scripts/verify/manifest.json`：无任何 4.x 条目，也不存在 `scripts/verify/checks/task-4*.mjs`，聚合命令 `pnpm verify:task -- 4.1 … 4.10` 返回 "unknown task id: 4.x"（退出码 1）。→ 需为 4.1–4.10 登记实际检查命令。
  - P0-2 `crates/echo-core/src/application/`：缺少用例层——无根切换编排（Prepare→Quiesce→Commit→Rebind 仅有域状态机）、无 `StartScan`/`CancelScan`/`ReconcileFsChanges`/`RelinkLibrary`、无 generation 扫描流水线/取消令牌/有界 worker/小批量 reconcile、无 size/mtime 快速跳过与 hash/音乐键重关联、watcher 无 target-claim 处理与全扫事件缓存。4.1/4.7/4.8 及 4.9/4.10 的用例部分未落实。
  - P0-3 质量命令失败：`cargo fmt --all -- --check`（多处差异）；`cargo clippy --workspace --all-targets --all-features -- -D warnings`（新文件约 25 个 lint 错误）。
  - P1 `infrastructure/filesystem/watcher.rs` 测试 `subscription_end_to_end_emits_a_stable_event` 无限挂起（`recv()` 无界阻塞，5 秒截止无法触发），`cargo test -p echo-core --all-features` 无法完成。
  - P2 根 `package.json` 无 `lint` 脚本，前端质量链需用 `pnpm --filter @echo/desktop …` 形式（既有约定，非阶段 4 回归；lint/typecheck/test/build 经过滤后均通过）。
  - P2 复核期间检测到**并发写入会话**正在编辑同一工作树（`watcher.rs`/`cover.rs` 于复核中途被修改；随后观测到另一会话持续新增 `application/scan.rs`、`relink.rs`、`root_switch.rs`、`watch.rs` 并登记 manifest 4.x 条目）。
- 编排决定：复核① FAIL 后暂不派发修复 subagent——存在活跃并发写入者，两个写入者会互相破坏。先等待工作树静默（≥10 分钟无任何文件改动）再重派复核（复核②）。若并发会话停在工作未完成状态，再派发修复 subagent（§5.3）。
- 修复轮次：0（并发实现会话自行补齐；本轮未派发修复 subagent）
- 复核②（独立验收 subagent，全新上下文，2026-08-29 04:07 派发）：
  - 结论：**PASS**
  - 独立运行通过：`pnpm verify:self-test`、`pnpm verify:task -- 4.1…4.10`（manifest 4.x 条目映射 56 个真实 Rust 测试，逐一核对非空壳）、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test -p echo-core --all-features`（188 个测试 0 失败，第 1 轮挂起的 watcher 测试已改为辅助线程+10 秒截止）、`cargo deny`、前端过滤链 lint/typecheck/test/build、fixtures 13/13 checksum、`openspec validate --strict`。
  - 规格同步核查通过：openspec/specs/ 七个能力规格与 change delta 逐字一致；tasks.md 仅勾选 4.1–4.10 十行；design.md/proposal.md 未动。
  - P2 遗留（不阻断，留待后续阶段择机处理）：
    1. `application/root_switch.rs:505` 测试未真正注册在飞扫描（取消路径已由 scan.rs 用例真实覆盖）；
    2. tasks.md 4.1「播放中切根」无直接测试——PlayerActor 属阶段 7/8，**递延记录：播放中切根集成验证递延至 8.x 运行时阶段**；
    3. `infrastructure/metadata/tags.rs:322` `write_oversized_fixture` 用 `mem::forget` 泄漏临时目录；
    4. `application/watch.rs:360` 与 `application/scan.rs:387` 约 40 行逐字重复；
    5. `infrastructure/filesystem/watcher.rs:392` debug 日志直接输出 notify 原始错误，可能内嵌路径（绕过 redact_path 约定）；
    6. 三份编排过程文件（ORCHESTRATOR_PROMPT/PHASE4_AGENT_PROMPT/ORCHESTRATION_LOG）为未跟踪产物，归档前需用户决定提交或清理。
- Deferred：播放中切根集成验证 → 8.x；上述 P2 1/3/4/5 为代码改进项，不阻断。
- Commit：baseline（本条所在提交）`Complete phase 4 tasks: media parsing, scanning and watching` → **0014445**

---

## Task 1 — 5.1–5.10 安全导入、删除与崩溃恢复

### 5.1 逐输入 PlanImport 与预留 OperationId/SongId

- 状态：✅ PASS（复核第 1 轮通过）
- 实现 subagent：新增 `application/import.rs`（PlanImport 用例、ImportOutcome/ImportBatchReport、最小编号命名、6 测试）、`ImportSourceReader` port 与 `LibraryFileSystem::stage`、`FakeImportSources` 替身、`scripts/verify/checks/task-5.mjs`、manifest 6 条命令。
- 复核①：**PASS**（独立运行 verify:task 5.1 / fmt / clippy -D warnings / cargo test 全部通过，194 测试含架构测试；manifest 非空壳；tasks.md 除勾选外无改动）。
- P2 遗留（指向后续任务范围，不阻断）：
  1. publish 成功后执行失败会覆写 journal 为 RolledBack 并释放 claim → 孤儿最终文件风险，5.5 恢复矩阵须改为保留 claim 或清理已发布文件；
  2. 5.1 journal 边 `Planned→DatabaseCommitted→Completed` 与 operation 状态机转移不兼容（内存替身不校验），5.5 落地逐资源状态链时必须替换；
  3. `ImportSourceReader::read` 全量入内存（非流式），5.3 流式复制时解决；
  4. 失败项目标名仍占用 taken_targets（保守不覆盖，仅效率问题）。
- Deferred：无（全部本地可验证项已验证）
- Commit：`Implement task 5.1: per-input PlanImport with reserved IDs` → **448335d**

### 5.2 默认命名、平台字符清理、短 hash 与最小冲突编号

- 状态：✅ PASS（复核第 1 轮通过）
- 实现 subagent：`plan_named_target`（歌手/歌手 - 歌曲名.扩展名 + 逐个探测 `(n)`）、domain/text 三平台 golden 模块与 UNKNOWN_ARTIST/UNNAMED_SONG 兜底、`MetadataReader::read_bytes`（导入前从源字节读标签）、fake publish create-new 冲突检查、manifest 16 条命令；并修正 2.3 遗留的「空白标签落错兜底」缺陷。
- 复核①：**PASS**（verify:task 5.2/5.1、fmt、clippy -D warnings、cargo test 全部独立复跑通过；三平台 golden 为纯函数断言无伪造递延；计划后竞态测试证明绝不覆盖；tasks.md 未被动）。
- P2 遗留（不阻断）：
  1. `domain/text.rs build_target_path` 与 application `plan_named_target` 双实现存在漂移风险（测试交叉断言一致，建议复用）；
  2. `batch_state` 每批全根 enumerate，O(曲库规模)/批，12.5 性能任务时评估；
  3. `path_identity_key` 大小写折叠使 Linux 大小写可并存文件被编号而非共存（保守方向，golden 已记录）。
- Deferred：无（三平台命名规则均为纯函数 golden，已本机全部通过）
- Commit：`Implement task 5.2: import naming rules with platform golden cases` → 见 git log

### 5.3 专属受控暂存、流式复制+BLAKE3、exclusive 目标保留与原子 publish

- 状态：✅ PASS（复核第 2 轮通过）
- 实现：5.3 的实现草稿由并发会话留在工作树（未提交、未登记），编排接管后作为 round-0 实现候选纳入复核循环。实现内容：`application/import.rs` 的 stage-and-plan / staged copy / BLAKE3-during-stream / 原子 publish 流程、`ImportSourceReader`/`LibraryFileSystem` port 的 stage/read_staged/discard_staged/publish、SQLite 条件唯一 target claim、exclusive create-new 占位 + fsync + rename 每文件原子发布；测试覆盖专属受控暂存、逐资源 source/staging/target/hash、DB 仅完整音频发布后可见、源文件不变、同名用户目录绝不被写入。
- 复核①（独立验收）：**FAIL**。
  - P0-1 `scripts/verify/manifest.json`：5.x 段只到 5.2，无 5.3 条目，`pnpm verify:task -- 5.3` 报 `unknown task id: 5.3`（退出码 1）。→ 需为 5.3 登记实际检查命令（任务 1.3 红线：未登记不得宣称完成）。
  - P2（不阻断）1) publish 空占位窗口可被 watcher 短暂观察；2) 目录 fsync advisory 平台降级未登记说明；3) 测试替身不注入读取中途截断（归属 5.5 故障注入范围）。
- 修复①（§5.3 修复 agent）：在 `scripts/verify/manifest.json`「5」组 5.2 后新增 id=5.3，登记 5 条命令（`task-5.mjs 5.3 <test-name>`，对应 5 个核心测试）；三条 P2 按「不得顺手重构无关代码」未改动。修复后 `pnpm verify:task -- 5.3` ok、fmt/clippy 干净、`cargo test` 220 通过 0 失败、未破坏 5.1 登记。改动仅 manifest.json（+26 行）。
- 复核②（独立验收，全新会话）：**PASS**。独立重跑 `pnpm verify:task -- 5.3`（manifest 5 条命令逐条真实指向非空壳测试并逐一通过）、fmt、clippy -D warnings、`cargo test` 220 通过；9 个验收点全部有真实实现与直接测试证据；架构红线（echo-core 纯净、infra 只实现 ports、错误脱敏、tempdir-only 测试、无 specs/design/schema 漂移）全部通过；无范围外改动。
- P2 遗留（不阻断，登记）：1) publish 占位窗口（visibility nuance，建议在 port 文档注释说明）；2) 目录 fsync advisory（文件级 fsync 为硬性；建议恢复期二次核对已 rename 文件）；3) 测试替身不注入中途读取截断（5.5/5.6 故障注入时补充）。
- Deferred：无（全部本地可验证项已验证通过）。
- Commit：`Implement task 5.3: dedicated controlled staging, streaming copy with BLAKE3, exclusive target reservation, fsync and atomic per-file publish` → 见 git log

---
