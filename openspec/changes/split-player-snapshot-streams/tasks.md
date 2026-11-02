## 1. 规格基线

- [ ] 1.1 复核本 change 的 delta 与主规格的对应关系：`desktop-playback` 新增 4 条 Requirement（归档后顺延为 R19–R22）、修改 1 条（R03 播放队列面板展示）；`audio-effects` 新增 2 条 Requirement（归档后顺延为 R07–R08）。用 `git diff origin/main -- openspec/specs/` 确认 propose 阶段**未**改动主规格
- [ ] 1.2 运行 `pnpm exec openspec validate split-player-snapshot-streams --strict`，确认通过
- [ ] 1.3 运行 `pnpm exec openspec validate --archived`，确认既有归档规格未被破坏

## 2. 队列修订号（DP-R20）

- [ ] 2.1 在 `crates/echo-desktop/src/player/queue.rs` 新增 `QueueRevision(u64)` 新类型（参照 `crates/echo-desktop/src/ipc/events.rs` 的 `CatalogRevision` 写法）与 `Queue::revision()` 读取接口
- [ ] 2.2 在 `Queue` 的每个会改变投影的 mutator 中推进修订号：`push` / `insert_next` / `promote_to_priority` / `remove` / `remove_song` / `remove_direct` / `set_current` / `advance_next` / `advance_in_mode` / `advance_priority` / `previous` / `previous_in_loop` / `clear_pending` / `set_blocked` / `set_priority` / `set_shuffle` / `refresh_shuffle_bag` / `restore_history`。允许无效调用也推进（宁可多发一次，不可漏发）
- [ ] 2.3 在 `crates/echo-desktop/src/player/coordinator.rs` 增加私有辅助 `replace_queue(&mut self, queue: Queue)`，在 `play_context` / `play_context_paused` / `restore_session` / `play_temporary` 四处整体替换队列时搬运旧修订号，避免归零（DP-R20-S04）
- [ ] 2.4 在协调器侧为自身持有的状态推进修订号：本轮失败集合的增删（`on_load_error` 与四处 `failed_round.clear()`）与播放模式切换（`set_mode` 与 `prime_context_paused`）（DP-R20-S03）
- [ ] 2.5 补单元测试 `queue_revision_advances_on_every_projection_changing_mutation`（逐个 mutator 断言修订号递增）、`queue_revision_survives_whole_queue_replacement`（DP-R20-S01–S05），运行 `cargo test -p echo-desktop --all-features`

## 3. 队列投影的 O(n) 改造（DP-R21）

- [ ] 3.1 把 `Queue::view_entries()` 的成员资格判定与条目查找改为**调用内局部索引**：`HashMap<&QueueEntryId, &QueueEntry>` 供查找，`HashSet<&QueueEntryId>` 供 `pending` / `shuffle_bag` / `emitted` 判定；不得引入需要跨 mutator 同步维护的常驻索引
- [ ] 3.2 补测试 `queue_projection_stays_within_budget_for_ten_thousand_entries`：构造 10000 条目的队列并断言投影构造在 500 毫秒内完成（DP-R21-S01）；裕度须足以在两核 CI runner 上稳定通过
- [ ] 3.3 补/复用回归测试 `queue_projection_order_and_content_unchanged`，确认当前项置顶、随机轮次顺序、优先区前置、被阻塞条目不出现等既有语义逐字不变（DP-R21-S02）
- [ ] 3.4 运行 `cargo test -p echo-desktop --all-features`，确认 `player::queue` 相关用例全部通过

## 4. 传输分层：后端（DP-R19、DP-R22）

- [ ] 4.1 在 `crates/echo-desktop/src/runtime/player/mod.rs` 拆分快照契约：`UiPlayerSnapshot` 去掉 `queue` 与 `effects` 字段、新增 `queue_revision`；新增队列投影 DTO（`revision` + `entries: Vec<UiQueueEntry>`）
- [ ] 4.2 在 `crates/echo-desktop/src/runtime/player/snapshot.rs` 改造转发线程：位置流只映射传输状态与当前条目标识，不再构造队列投影；队列投影仅在修订号变化时构造
- [ ] 4.3 改造 `apps/desktop/src-tauri/src/main.rs` 的 `queue_provider`：**一次加锁**内读修订号，修订号未变则直接返回「无变化」，变了才在同一次持锁期间构造投影；顺带消除当前每帧两次 `coord.queue_view()` 的重复克隆
- [ ] 4.4 当前条目的展示元数据改为单条缓存查询（`QueueMetadataResolver` 增加单条查询接口），整队元数据解析只在投影构造时进行
- [ ] 4.5 在 `apps/desktop/src-tauri/src/main.rs` 发射 `player://queue` 与 `player://effects` 两个新事件；`player://snapshot` 的载荷不再携带队列与音效
- [ ] 4.6 新增 `player_sync` 命令（无参数、返回 void）：处理时重新下发当前队列投影与当前音效快照；注册进 `apps/desktop/src-tauri/src/bootstrap.rs` 的 `tauri::generate_handler!` 表
- [ ] 4.7 补测试 `snapshot_payload_carries_no_queue_entries_or_effects`、`snapshot_forwarder_emits_queue_projection_only_when_revision_changes`（DP-R19-S01–S04），运行 `cargo test -p echo-desktop --all-features`
- [ ] 4.8 运行 `pnpm --filter desktop generate:ipc` 重新生成 `apps/desktop/src/ipc/ipc-types.generated.ts`（**不得手工修改**），确认生成物与 Rust 侧一致

## 5. 音效快照的变更驱动（AE-R07、AE-R08）

- [ ] 5.1 修正 `crates/echo-desktop/src/effects_service/mod.rs` 的 `capture()`：`snapshot_sequence` 由「捕获计数」改为「内容变更计数」——与上一次捕获的状态比较（忽略序号字段本身），内容不同才推进序号（AE-R07-S03、AE-R07-S04）
- [ ] 5.2 在 `crates/echo-desktop/src/effects/` 内把派生的频响分析按 `(音效文档, 处理环境)` 记忆化：`analysis` / `presets` / `response_points` / `editable_bands` 复用既有输入，文档或环境变化时重算；记忆化不得放在 IPC 层（AE-R08-S01–S03）
- [ ] 5.3 在 `crates/echo-desktop/src/effects/math.rs` 增加**仅测试可见**的峰值扫描调用计数，补测试 `effects_analysis_is_not_recomputed_for_unchanged_document_and_environment`（断言位置帧不触发扫描）与 `effects_analysis_matches_recomputation_for_same_inputs`（断言记忆化不改变结果）（AE-R07-S01、AE-R08-S02、AE-R08-S03）
- [ ] 5.4 确认音效快照只在内容变化时下发：位置帧不重复下发（AE-R07-S01）、配置或生效状态变化时整份下发且序号严格递增（AE-R07-S02）
- [ ] 5.5 运行 `cargo test -p echo-desktop --all-features`，确认 `effects` 相关用例全部通过；确认 `audio-effects` 既有规格的行为与测试无回归

## 6. 传输分层：前端（DP-R19、DP-R22、DP-R03）

- [ ] 6.1 在 `apps/desktop/src/player/playerStore.ts` 增加独立的队列槽位与 `publishQueue()`，并导出 `usePlayerQueue()`；`publish()` 不再重建队列数组，位置更新保持队列数组标识不变
- [ ] 6.2 改造 `startPlayerEvents()`：**先**订阅 `player://queue` 与 `player://effects`，再订阅 `player://snapshot`，随后调用一次 `player_sync`；收到过期修订号或过期序号的载荷时忽略（DP-R22-S02、DP-R22-S03）
- [ ] 6.3 把 `apps/desktop/src/features/player/QueuePanel.tsx` 改为读取队列槽位而非 `snapshot.queue`
- [ ] 6.4 队列面板改为**窗口化渲染**：按既有行高基准与滚动偏移只渲染可视行及有限前后余量，上下用占位撑出总高度；队列总条目数仍显示（DP-R03-S05、DP-R03-S06）
- [ ] 6.5 补前端测试：位置流持续更新时队列数组标识不变、面板渲染行数与队列长度解耦、过期修订号被忽略、订阅顺序与 `player_sync` 调用（DP-R19-S04、DP-R22-S01–S03、DP-R03-S05）
- [ ] 6.6 运行 `pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`，确认全部通过

## 7. 静态检查与全量验证

- [ ] 7.1 运行 `cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] 7.2 运行 `cargo test --workspace`
- [ ] 7.3 运行 `pnpm --dir apps/desktop test:e2e`，确认播放队列面板的端到端路径无回归
- [ ] 7.4 真机复测并把数据写进 PR 描述：固定条件（本机 480 首曲库、前台播放、同一播放状态、相同采样时长与间隔）下记录 `footprint -p <pid>` 的总量与 `Malloc Large` 分量；确认播放态下不再出现 50–80 MB 的周期性回落
- [ ] 7.5 真机复测 CPU：`sample <pid> 5` 抓取 `echo-snapshot-forwarder` 线程，确认落在 `peak_gain` 的采样占比相对改动前（203/220）显著下降
- [ ] 7.6 运行 `pnpm exec openspec validate split-player-snapshot-streams --strict`

## 8. 场景登记与归档

- [ ] 8.1 在 `scripts/verify/scenario-commands.mjs` 为新增的 21 个场景（DP-R19 4 条、DP-R20 5 条、DP-R21 2 条、DP-R22 3 条、AE-R07 4 条、AE-R08 3 条）填入已实现且可执行的 command 与 evidence，再运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重新生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*.yaml`（**不得手工编辑 manifest**）
- [ ] 8.2 同步 `docs/traceability.md`，运行 `node scripts/verify/reconcile-scenarios.mjs` 确认 `spec = trace = manifest` 三方一致、无 mismatches
- [ ] 8.3 运行 `node scripts/verify/validate-scenario-commands.mjs` 与 `node scripts/verify/check-scenario-churn.mjs`，确认命令有效且未突破冻结上限
- [ ] 8.4 **独占运行** `pnpm verify:governance` 完整门禁，确认全部检查通过（该门禁须独占运行，并行 cargo 或其它扫描会让其中的墙钟断言误红）
- [ ] 8.5 归档本 change（`openspec archive`），确认主规格新增 5 条 Requirement、场景三方一致、`pnpm exec openspec validate --archived` 通过
