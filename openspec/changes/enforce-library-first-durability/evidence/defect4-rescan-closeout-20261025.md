# 缺陷 4 与重扫候选（5.2·F6）收尾记录（2026-10-25）

本文件记录缺陷 4（本分支引入的 echo-sync `ledger_protocol` 回归）与 5.2·F6（无变化重扫不生同步候选）的行为与验证收尾。状态导航见[交接](handoff-20261025.md)，进度勾选以 [tasks.md](../tasks.md) 为准。

## 缺陷 4：`cfg(test)` 豁免在跨 crate 集成测试中失效

- 根因（见交接"缺陷 4"节）：`sync/durability/commit.rs::commit_prepared_projection` 的 `#[cfg(test)]` 提交豁免只在 echo-core 自身编译为测试目标时成立；echo-core 作为依赖被 `echo-sync` 集成测试编译时为假，裸 `SqliteDatabase::open` 未装控制面 ⇒ `Unavailable { resource: "library durability" }`，`ledger_protocol` 2 例全红、治理门 exit 101。
- 修复选择**方向二（测试安装真实控制面）**，不放宽 `#[cfg(test)]`：该豁免的语义是"echo-core 自身单测的隔离仓储契约"，用运行时探测"是否在测试二进制里"放宽会削弱生产防漏装保护。测试侧安装真实文件系统控制面与生产路径一致，也是 `crates/echo-core/tests/sync_cursor.rs::prepared_replica` 的既有模式。
- `crates/echo-sync/tests/ledger_protocol.rs` 新增 `bind_library_database(database, music_root, binding, is_host)` 统一换装：`RootRegistry::register` → `RootControlPlane::write_manifest` → `database.configure_library_control(control)` → `LibraryRepository::upsert` 根 → `bind_sync_library`。manifest 的库 ID 是 `domain::library::LibraryId`（与 binding 用的 `domain::sync::LibraryId` 不同类型），经 `from_uuid(binding.library_id.as_uuid())` 换型免 uuid 依赖。三个调用点（host、replica（client 特性）、`verify_snapshot_enrollment` 的 enrollment 库）全部换装；原裸 `bind_sync_library` 路径删除。

## 5.2·F6：无变化重扫不生候选

- 根因：`application/scan/durable.rs` 早退分支（业务字段未变）`return project()` → project 闭包的 `tx.upsert_song`（`application/scan/mod.rs::persist_parsed_index`）→ `statements.rs::upsert_song` 无条件 `enqueue_object_snapshot` → `sync/sync.rs::enqueue_sync_in_root_with_resources` 无条件插入 `sync_mutations`/`sync_outbox` 并推进 `sync_local_highwater.local_revision`——违反 `local-library` delta"无变化重扫不增加 revision、播放计数或同步候选"。
- **为何不早退整体跳过 `project()`**：闭包同时刷新歌词候选/封面等本地事实，跳过会丢这些刷新；正确切口在同步发射点。
- **为何不在 enqueue 层全局去重**：首轮实现（`enqueue_sync_in_root_with_resources` 内按 digest 去重）让两条既有用例转红——`sync_concurrent_sequential_submissions_stay_monotone`（8 次并发 favorite toggle 期望 8 条 fact，并发下相邻同值写入 payload 相同被吞，7/8）与 `initial_baseline_preserves_chain_current_state_and_undoable_song`（`ensure_host_sync_baseline` 对每首歌重发 song/favorite/play-stats 三个相同 payload 的快照，3/4）。显式编辑与 baseline 重发必须无条件铸造，已回退该实现。
- 采用**发射点守卫**：`statements.rs::upsert_song`（重扫/导入快照的唯一发射点）改调新函数 `sync::enqueue_song_snapshot_if_changed`——取该对象按 `local_revision DESC` 最新一条 mutation 的 `(local_revision, payload_digest, status)`，digest 相同且 `status != 'rejected'` 时返回既有 revision（不推进 highwater、不插 mutations/origin cursor/outbox）；否则照常铸造。digest 计算抽成共用 helper `snapshot_digest`（blake3 over kind\0object\0operation\0payload[\0resources]，与 enqueue 落库口径完全一致）。rejected 尾巴不去重，保留修复路径。favorite/歌单/成员/统计/baseline 等其余发射点保持无条件铸造。
- RED→GREEN：`infrastructure::sqlite::sync::queue::tests::unchanged_reupsert_does_not_mint_another_candidate_or_advance_the_revision`（修前 RED：left 2 / right 1）。断言：无变化重 upsert 后 pending 队列仍 1 条且 `mutation_id` 不变、`sync_local_highwater.local_revision` 仍 1、该对象 `sync_outbox` 仍 1 行；改 title 后仍铸造第 2 条且 `origin_seq`/`previous_mutation_id` 正确链接（去重是 payload 精确匹配，不是节流）。
- 扫描层组合关系：`persist_parsed_index` 的 project 闭包在 durable 资源在场时执行 `tx.upsert_song`（早退分支即触发此路径，现被守卫覆盖）；无 durable 资源的刷新路径走 `refresh_song_file_index`（本就不入队）。`ScanFixture` 是 MemoryDatabase fake，同步候选只能在 SQLite 层断言，故用例落在 queue 测试模块。

## 验证命令（仓库根目录，2026-10-25）

| 命令 | 目标 | 结果 |
| --- | --- | --- |
| `cargo test -p echo-sync --all-features` | lib | 65 passed / 0 failed |
| 同上 | arch | 1 passed / 0 failed |
| 同上 | discovery_network | 3 ignored（需 mDNS 组播环境，既有约定） |
| 同上 | ledger_protocol | **2 passed / 0 failed**（缺陷 4 验收口径） |
| `cargo test -p echo-core --lib` | lib | 870 passed / 0 failed / 1 ignored（共 871，较 4.2a 收尾 +1 即新用例；ignored 为既有性能基准） |
| `cargo test -p echo-core scan` | lib | 52 passed / 0 failed |
| `cargo test -p echo-desktop`（全目标） | lib 及集成 | 443 / 2 / 1 / 1 / 1 / 4 / 3 passed，0 failed |
| `cargo clippy -p echo-core --all-targets --all-features` | — | 0 警告 |
| `cargo fmt --all --check` | — | 通过 |

## 范围与遗留

- `pnpm verify:governance` 修复后首次整门复跑（2026-10-25 10:10–11:07 后台）**exit 101 但失败点已离开缺陷 4**：`cargo test --workspace` 阶段 `echo-app` 的 `native_pull_drains_more_than_one_commit_page_without_losing_local_work` 超 60 秒被 SIGTERM（下载侧 513 首歌既有性能边界，见 [native-513.md](native-513.md)）；`ledger_protocol` 不再失败。当日用户决策：513 用例退出治理路径（标 `#[ignore]` 保留代码），功能验证先用少量数据通过，速度后续优化；`cargo test -p echo-app --bin Echo` 复验 132 passed / 0 failed / 2 ignored。
- **第二次整门复跑（bqyjobr4a）exit 1，失败点转移至 coverage gate（task-12.8）**：89.69% < 90%（3323/32219 行未覆盖）。定责为本分支新增 `sync_state` 模块稀释（`related.rs` 的 `song_alias`/`member_alias` 两函数整体零测试为最大源）；修复分两轮补测（song_alias/member_alias 针对性测试，及 receipt 全真链路两幕 + conflict 双拒绝分支），定责与修复全程见 [tasks.md](../tasks.md) 11.2 注记。**过程警示：两次后台 coverage 复测的任务通知均报 exit 0，实为 `命令; echo "EXIT: $?"` 包装里 echo 自身的退出码（输出文件内真实标记 `EXIT: 1` / `COV EXIT: 1`）——89.93%/89.94% 均未过线，"门过"系误记并已在 11.2 更正。** 最终 coverage 数字与第三次整门复跑结果以 tasks.md 11.2 为准；第三轮补测（stray receipt fact、conflict 行 resolved 幂等）后已裸跑真实 exit 0：90.01%，missed 3219/32219（covered 29000 ≥ 28998），coverage gate 过线。
- 5.2 其余缺口未动：扫描后 wipe 重建用例、扫描侧重复事件专项、真实只读卷扫描用例、改名用例不被 `cargo test -p echo-core scan` 过滤器命中的问题（见 tasks.md 5.2 注记），5.2 保持未勾选。
- 缺陷 2（歌单投影 `created_at` 伪造 / `updated_at` 不推进）仍未修，归属未定（拟并入 6.1 或投影正确性工作）。
