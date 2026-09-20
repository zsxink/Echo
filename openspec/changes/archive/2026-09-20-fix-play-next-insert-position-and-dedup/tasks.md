## 1. 优先区物理偏移修复与不变量

- [x] 1.1 修正 `crates/echo-desktop/src/player/queue.rs` 的 `insert_next`：把物理插入点从 `current_index + 1 + priority.len()` 改为「优先区尾部」——先定位当前项之后的第一位，将 `priority` 中既有优先项连续占用切片，把新 entry 物理放到该切片末尾，再 `priority.push(id)`。验证：`cargo test -p echo-desktop queue::` 全绿，且新增/既有 `view_projection_reflects_insert_next_and_clear_pending` 测试通过
- [x] 1.2 为新不变量写单元测试：队列 [当前, A, 普通1, 普通2] 上依次 `insert_next(B)`、`insert_next(C)` 后物理顺序为 [当前, A, B, C, 普通1, 普通2]，`priority_ids()` == [A, B, C]，且再 `clear_pending` 后仅剩当前项。验证：`cargo test -p echo-desktop queue::priority_lane_physical_contiguity`
- [x] 1.3 验证移除/失败跳过路径不破坏物理连续：对含优先区的队列 `remove` 一个优先项、一个普通待播项后，`priority_ids()` 保留顺序、`current_index` 指向正确、`view_entries()` 当前项置顶且无重复。验证：`cargo test -p echo-desktop queue::remove_keeps_priority_lane_consistent`

## 2. 去重与提升：Queue 身份匹配

- [x] 2.1 在 `Queue` 新增按 `QueueItem` 身份匹配的辅助（`Library(SongId)` 按 song 相等；`Temporary` 按 `path` 相等），返回队列中首个匹配 entry 的 id。验证：单元测试覆盖库内歌曲与临时项两种判据
- [x] 2.2 新增 `Queue::promote_to_priority`（或等价私有入口）：目标 item 已在队列中时，把该既有 entry 从 `entries` 物理位置移除并重插到优先区最前头（`current_index + 1`），`priority` 中去掉重复 ID 后前置该 ID；不在队列中则退化为追加新 entry。验证：去重 + 提升 + 重复优先项收敛的单元测试通过
- [x] 2.3 保持普通入队不折叠：确认「加入播放队列」走 `push` 不经过新逻辑，且对同一歌先 `push` 两次再 `promote_to_priority` 只提升一份，另一份仍是普通待播。验证：`queue::` 单测覆盖「提升不折叠普通副本」

## 3. 协调器与 Tauri 接入

- [x] 3.1 `coordinator.rs::play_next` 改为调用去重 + 提升路径（`priority` 里已存在目标 item 时提升、否则追加）。验证：协调器测试覆盖「对已在普通待播的歌点下一首 → 提升到优先区最前」「对不在队列的歌点下一首 → 追加」两种输入
- [x] 3.2 `apps/desktop/src-tauri/src/commands.rs` 的 `queue_command` `playNext` 分支保持传参结构 `songId` 不变，确认返回类型仍为 `Result<(), IpcErrorDto>`，前端 `fireAndForget` 调用方无需改动。验证：`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过，前端 `pnpm typecheck` 通过

## 4. 会话恢复与收敛

- [x] 4.1 确认 `rebuild_queue` + `set_priority` 在恢复旧会话时继续丢弃未知/重复/阻塞/当前 ID，且不会因新物理连续要求重排恢复到错误位置。验证：`runtime/player/tests/restore_prime.rs` 既有恢复测试通过
- [x] 4.2 补充「恢复后优先区含重复副本时按设想起收敛」的测试：恢复含同一歌曲两个 priority ID 的会话，首次 `insert_next`/消费后该歌收敛为单一份于优先区最前。验证：`cargo test -p echo-desktop`（queue 单测 + restore_prime）全绿

## 5. 前端与终验

- [x] 5.1 同步 `apps/desktop/src/bridge/index.ts` 的 `queue_command` 声明（若返回语义未变则仅确认类型断言 `IpcCommandResultMap` 与 `BridgeCommandArguments` 不漂移）。验证：`pnpm typecheck` 通过
- [x] 5.2 全量回归：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test`、前端类型检查全部通过；对照归档 `refine-playback-queue-semantics/design.md` 的 FIFO 语义逐条核对本文档「Decisions」一致