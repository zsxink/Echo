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

### 5.4 同名 `.lrc` 可选子资源与独立结果

- 状态：✅ PASS（复核第 1 轮通过）
- 实现：`ImportSourceReader` 新增 `sidecar()`/`open_sidecar()` 端口方法与 `SidecarInfo` 值类型（核心只接触逻辑句柄与字节流，真实路径留在桌面可信边界）；`import.rs` 新增 `LyricsImportResult`（Imported/None/Failed）独立结果、`plan_lrc`/`publish_lrc`/`lrc_target_of`/`journal_lrc_item`，音频验证后 best-effort 发布侧车，`commit` 仅在自有侧车发布成功时才接 sidecar candidate（失败即清除，不嫁接任意 `.lrc`）。新增 8 个 5.4 测试；manifest 按任务 1.3 登记 id=5.4（8 条命令）。
- 复核①（独立验收，全新会话）：**PASS**。独立重跑 `pnpm verify:task -- 5.4`（8 条登记命令逐条通过）、5.1–5.3 无回归、self-test、fmt、clippy -D warnings、`cargo test` 215 通过；四个验收点（嵌入歌词优先 / LRC 成功配对最终基础名含编号、真实临时目录复制源不变 / LRC 失败"音频成功歌词失败"不留半侧车三路径 / 独立结果结构）均有真实非空壳测试证据；架构红线全过；无范围外改动。
- P2 遗留（不阻断，登记）：1) 测试替身 reader 需要显式 `add_sidecar` 注册而不自动按基名发现 `.lrc`（与 port 契约一致，桌面 reader 延迟到 IPC 任务）；2) 真实 fs 栈仅 single manifest 命令覆盖，失败路径（不可读/冲突/大小不匹配）经替身演练（可接受）。
- Deferred：崩溃恢复矩阵 → 5.5；真实桌面 sidecar reader → 桌面 IPC 任务（7.3 等）。均已如实标注，未宣称已通过。
- Commit：`Implement task 5.4: same-basename optional .lrc sub-resource with independent per-input result` → **17b62d9**

### 5.5 导入 journal 逐资源恢复矩阵与故障注入

- 状态：✅ PASS（复核第 1 轮通过）
- 实现：工作树中的 round-0 实现候选（沿用 5.3 收编先例，编排接管为 round-0）。`application/recover.rs`（1074 行，`RecoverOperations` 用例）：逐资源 Copy/Validate/Publish `Pending→Applied` 恢复、target claim 生命周期、三位置存在性/hash 恢复矩阵；`application/ports.rs` 新增 recovery 专用 port（`OperationJournalRepository::incomplete_items`、`LibraryFileSystem::discard_staging_path`/`path_exists`/`publish_from_staging_path`，后两者要求校验路径须解析进 Echo 专属 marker staging 区，拒绝外来路径）；`infrastructure/filesystem/adapter.rs` 与 `sqlite/` 对应实现；`memory_database.rs`/`repositories.rs`/`small_fakes.rs`/`filesystem.rs` 替身扩展。manifest 登记 id=5.5（6 条命令，task-5.mjs 5.5 <test> 逐条指向真实故障注入测试）。
- 复核①（独立验收，全新会话）：**PASS**。独立重跑 `pnpm verify:task -- 5.5`（6 条命令逐条真实非空壳并逐一通过）、fmt、clippy -D warnings、`cargo test -p echo-core --all-features` 全部通过；故障注入覆盖 state-write Before、publish/rename Before+After、DB commit Before+After、copy Before、mid-read truncation、watcher 抢占，全部恢复两次并断言唯一终态/同一预留 UUID/无孤儿最终文件/无重复/无幽灵记录；架构红线全过，specs/tasks/design 未动，无越界实现 5.6/5.7/5.8/5.10；deferred（5.10 runtime 启动接线）如实标注未宣称已通过。
- P2 遗留（不阻断，登记）：1) state-write After 相未逐点独立注入（由下一点 Before 传递覆盖）；2) Copy After（无 journal envelope 的暂存残留）未直接测试；3) fsync 失败未直接注入（经 copy/publish 路径覆盖）；4) 真实 sqlite `incomplete_operation_items` 与 adapter `publish_from_staging_path`/`discard_staging_path` 集成路径未在 5.5 内用集成测试演练（现行 manifest 测试走替身）。前 3 项归 13.3 故障注入报告范围；第 4 项建议后续补集成测试。
- Deferred：5.10 runtime ready 前的 `RecoverPendingOperations` 启动接线 → 5.10。已如实标注。
- Commit：`Implement task 5.5: import journal per-resource recovery matrix with fault injection` → **27206c2**

### 5.6 双重 BLAKE3 去重与幂等重试

- 状态：✅ PASS（复核第 1 轮通过）
- 实现：`application/import.rs` 在 publish 后、`PublishApplied` 提交前新增二次全文件 BLAKE3 去重检查 `pre_commit_duplicate()`（重新查询曲库同 hash 歌曲，排除自身预留 ID——防并发 watcher 已用预留身份应用）；命中则返回 `ImportOutcome::Duplicate`，恢复 arm 只删除"仍携带自身内容 hash"的已发布重复文件（禁删外来文件）、回滚两条 journal item、释放 claim。`LibraryFileSystem` 新增 `discard_published` port（根约束、拒 symlink、幂等），adapter/fake/recover 实现对应。计划时点去重（5.3/5.5 已有）保留。manifest 登记 id=5.6（2 条命令，指向真实竞态/幂等测试）。计划时点去重此前已存在于 `stage_and_plan`；本任务补齐提交前第二道检查，满足 design §8「计划时和提交前各检查一次」。
- 复核①（独立验收，全新会话）：**PASS**。独立重跑 `pnpm verify:task -- 5.6`（2 条命令真实非空壳并通过）、fmt、clippy -D warnings、`cargo test` 223+6+7 全绿；竞态测试真实模拟「两检查点之间并发导入提交相同内容 → 恰好一个逻辑歌曲」，幂等重试证明重试不新增歌曲/不重复复制；架构红线全过、specs/tasks/design 未动、无越界（`discard_published` 的 symlink 拒绝与 4.2 边界一致）。
- P2 遗留（不阻断，登记）：1) 预提交去重把 DB 查询失败视为"无重复"（有意为之：此处失败会孤儿化已发布且校验通过的完整文件；文件仍以预留 UUID 提交，正确性保留）；2) `execute` 的 `#[allow(clippy::too_many_lines)]`（风格）。
- Deferred：无。实现者自评提及的「rollback 与 release_claims 之间崩溃可留 claim 未释放」为 5.5 前既有范围外问题，登记待 13.3 检查。
- Commit：`Implement task 5.6: dual BLAKE3 dedup and idempotent retry for concurrent import` → **38febb0**

### 5.7 Echo 主动删除、专属 trash 暂存与 10 秒 undo

- 状态：✅ PASS（复核第 2 轮通过）
- 实现（round-0 中途草稿收编 + 续做补全）：首个实现会话因 API 限流中断留下未完成的 `application/delete.rs` 草稿（`DeleteSongs` 删除 + `RestoreDeletedOperation` 恢复，含 6 测试），续做会话继承补全。`delete.rs`：逐资源 `StagePending→StageApplied` rename 进受控 `trash/<op-id>`（注入 `trash_path`）、pending-delete 单事务隐藏（隐藏时写 `undo_deadline=now+10s`）、10 秒 undo（`safe_restore_target` 编号恢复、保留 UUID/收藏/统计/歌单 position）、只读根拒删除、无 sidecar 只暂存 audio。`recover.rs`：delete 恢复矩阵（StagePending 规范 applied、RestorePending 规范 restored、两处证据矛盾 held 不删文件、mid-restore 崩溃补全到歌曲回 Available+release claims、过期隐画面交 TrashPending）。`ports.rs`/adapter/sqlite/testing 对应端口与持久化。manifest 登记 id=5.7（**14** 条命令：6 delete + 7 recover 崩溃矩阵 + 1 外来占用恢复 held）。
- 复核①（独立验收，全新会话）：**FAIL**。P1 `recover.rs:448-459 recover_restore_pending`：mid-undo 崩溃把 item 留 RestorePending 且原 target 被外来内容重新占用、暂存完好时，直接 `restore_from_trash` 到原路径命中 exclusive conflict，经 `?` 传播成硬错误**中止整个根目录恢复 run()**——与代码自身注释「foreign occupant is a conflict the caller holds」及 design 逐 item held 语义矛盾，也未复用 undo 的编号恢复 `safe_restore_target`；分支无测试。P2 `delete.rs:449-450`：占用→编号决策未与 recover 共享。数据安全保留，但为崩溃恢复矩阵真实健壮性缺陷。
- 修复①（§5.3 修复 agent）：P1 将 `restore_from_trash` 的 `Conflict` 映射为逐 item `FailedRecoverable` held（upsert+`Ok(false)`），不再经 `?` 中止；复用 delete.rs 非私有共享 helper `safe_restore_target`（live undo 与恢复共用同一编号决策，编号 1..=100 耗尽即 held）；held 项 claim 不释放、外来占用者与暂存均保留。P2 收敛共享 helper。新增第 14 条 manifest 测试 `foreign_occupancy_at_recover_restore_pending_holds_the_item_not_the_whole_run`（A 外来占用/编号耗尽 → held 不中止，B 普通操作仍独立恢复）。
- 复核②（独立验收，全新会话）：**PASS**。确认 P1 修复正确（held 经 `recover_delete_items`/`recover_delete_operation` 不释放 claim，`run()` 继续处理无关操作）、P2 收敛到位、新测试真实非空壳断言到位；四项命令全部真实通过（`pnpm verify:task -- 5.7` 14 条 exit 0、fmt、clippy -D warnings、`cargo test -p echo-core` 237+6+7 全绿）；无越界实现 5.8/5.9/5.10（仅允许的 Hidden→TrashPending 过期面交）、无误改 specs/tasks/design、无平台项被宣称通过、路径无泄露、测试不进真实用户目录。
- P2 遗留（不阻断）：held 仅在 100 个编号候选耗尽时触发（常见外来占用现在编号恢复前进）；编号恢复后歌曲记录路径保持原路径，relink 在后续扫描完成（既有设计行为，未改）。
- Deferred：Trash 前滚与 SystemTrashPort 调用 → 5.8；外部 missing 区分 → 5.9；启动协调 → 5.10。均已如实标注，未宣称已实现。
- Commit：`Implement task 5.7: Echo-internal per-resource delete stage/restore with 10s undo and crash-safe recovery` → 见 git log

---
