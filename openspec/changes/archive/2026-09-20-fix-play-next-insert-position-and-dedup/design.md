## Context

动机与范围见 [proposal.md](proposal.md)。当前实现位于 `crates/echo-desktop/src/player/queue.rs` 与 `coordinator.rs`，随 `refine-playback-queue-semantics` 归档设计引入了 `priority: Vec<QueueEntryId>` 优先区。

现状要点：

- `Queue::insert_next` 用 `current_index + 1 + priority.len()` 计算物理插入偏移：`Vec::insert` 会把该偏移处之后的既有项都右移，因此每插入一次 `current_index` 后面的物理位置就多一位，第二次点「下一首播放」落在第三首——这正是 issue 5 的「越插越靠后」。
- 优先区是 `priority: Vec<QueueEntryId>`（FIFO，消费顺序即优先顺序），`view_entries`/`advance_priority` 都按这个 ID 列表驱动，但 `entries` 物理向量与优先区消费顺序并不自动一致。归档设计说「物理插入到尾端以免循环遍历错位」，当前实现只在 `priority` 增长了，物理偏移也随之增长，导致二者不一致。
- `queue_command` 的 `playNext` 分支（`apps/desktop/src-tauri/src/commands.rs`）直接把 `lib_entry(id)` 交给 `coordinator.play_next`，无任何去重。
- 会话恢复 `rebuild_queue` 最后通过 `queue.set_priority(...)` 重建优先区——`set_priority` 已是「丢弃未知/重复/阻塞/当前 ID」的收敛实现。

## Goals / Non-Goals

**Goals:**

- 修复「下一首播放」越插越靠后的物理偏移错误。
- 为「下一首播放」加入以歌曲/临时项身份为判据的去重与提升：已在队列中的目标被提升到优先区最前头，而非新增重复 entry；不在队列中才追加。
- 保持「加入播放队列」等普通入队路径不变：同一首歌仍可多次出现。
- 保持优先区消费顺序（FIFO）与物理存储连续一致，且会话恢复 / 移除 / 重复清理不破坏该不变量。

**Non-Goals:**

- 不实现 UI 拖拽排序、移除单项、队列项编辑。
- 不改变播放统计、临时项持久化边界、资料库 UUID 或 mpv 适配。
- 不引入新的队列持久化格式（`priority` 仍按 entry ID 列表持久化）。
- 不为「下一首播放」变更 Tauri 参数结构（仍 `songId`），仅在需要时扩展返回语义。

## Decisions

### 物理插入重算为「当前项之后连续紧邻的优先区块」，而非 `current + 1 + priority.len()`

`insert_next` 的错误在于把 `len()` 直接相加进下标，而当 `Vec::insert` 在已有优先项之前插入时，旧优先项被右移、`current_index` 不变，于是下标越算越大。

修复：把优先区建模为 `entries` 中**紧邻当前项之后的一段连续切片**。插入新项时，先找到优先区头部 `insert_at = current_index + 1`，把该下标处（含）往后的既有项整体右移，并在优先区**尾部**物理压入新 entry（即 `priority.len()` 个位置之后）。由于 `view_entries`/`advance_priority` 已以 `priority` ID 列表为准，物理切片仅需保证：`current + 1 ..= current + priority.len()` 恰好是 priority 列表的顺序映射，如此「下一首播放 A、B、C」后普通待播自然落在 C 之后，列表循环回绕前不会穿越优先区。

- 替代方案 A（只是把每次插入固定到 `current + 1`）：连续 A、B 会得到倒序 B、A，违背归档 FIFO 语义，不采用。
- 替代方案 B（完全不做物理重排，仅维护 ID 列表，投影都走 `view_entries`）：`advance_priority` 依赖 `view_entries` 的重排，从消费结果看正确，但删除、失败跳过、恢复等路径对物理下标 `current_index` 的维护更脆弱，且归档设计明确要求「物理插入到尾端」保持一致，不采用。

### 去重 + 提升：以 `QueueItem` 身份匹配，而不是 `QueueEntryId`

「下一首播放」目标不在队列中时新建 entry；已在队列中时**不新建**，而是把既有 entry 提升到优先区最前头：

- 判据在 `QueueItem` 身份：`Library(song_id)` 按 `SongId` 相等；`Temporary` 项按路径身份（`path`）相等。这是 issue 5「同一首歌」的语义。**不得**按 `QueueEntryId` 判重——那样会因 entry ID 永不重复而必然失败。
- 找到任一匹配 entry 后，把它从 `entries` 物理位置移除，重插到优先区最前头（`current_index + 1`），并把其 ID 放到 `priority` 的首位；`priority` 中原有的该 ID 副本先删除再前插，确保唯一。
- 若该歌曲在普通待播区还有其它副本，它们不受影响（仍是普通待播项）；提升只作用于找到的第一份 + 去重 priority 中该歌的重复 ID。行为：同一首歌反复点「下一首播放」→ 始终只有一份位于优先区最前，普通待播不逐次后推。
- `advance_priority` 消费时该优先项离开优先区，歌曲作为普通待播项继续存在于队列（与被提升前一致），因此「提升」不是删除。

为把「移除已有 entry + 重插 + 维护 current_index」封装在一个不变式清晰的入口，扩展一个 queue 内部新方法（例如 `promote_to_priority(item) -> Option<QueueEntryId>`），在 `Queue` 内完成 `priority`、`current_index`、`entries` 的同步一致性修改。

### 去重只作用于 `playNext`，普通入队保持可重复

`insert_next` 的调用方（`coordinator.play_next`）是唯一走去重+提升的路径。「加入播放队列」走 `push`，不受影响。`set_priority` / `set_next` 等恢复路径与去重无冲突——恢复时 priority 列表本身只存 entry ID，而 entry ID 永远唯一，去重不会把持久化中合法的重复歌曲折叠掉。

### `set_priority` 复用为「收敛优先区」

会话恢复的 `rebuild_queue` 已经调用 `set_priority`，它过滤掉未知/重复/阻塞/当前 ID。本 change 保留该行为，只确保其与新的物理连续不变量一致：恢复后如果 priority 指向的 entry 在物理上不位于 `current+1..` 连续区，不应额外搬运——`view_entries` 已按 priority 重排，物理恢复保持「追加顺序」即可。避免过度设计：不要求在恢复时重排整个 entries 向量，只保证 `insert_next` 之后物理切片与 priority 一致。

### Tauri 返回语义

现有 `queue_command` 返回 `Result<(), IpcErrorDto>`，前端 `fireAndForget` 不消费返回值。本 change 保持返回类型不变（`()`），避免不必要的桥契约变更——「返回被提升/新建的 entry ID」在 issue 5 验收不是必需，且会牵动桥声明与 `IpcCommandResultMap` 的类型断言。唯一的返回值相关需求是无重复、无错位的可观察行为，由队列投影体现。

## Risks / Trade-offs

- [物理偏移修复依赖 `priority.len()` 与物理切片的一致性，删除/失败跳过会同时改 `entries` 下标故可能漂移] → 所有物理下标变化（`remove`、`remove_song`、`clear_pending`、`advance`）已通过 `priority.retain` 同步清理 ID；新增/移动逻辑集中在 `Queue` 方法内，并用「优先区 = current 后连续切片」的单元测试锁死不变量。
- [去重把「已在普通待播区」的歌曲提升，可能让队列中相同歌出现两份（一份被提升 + 一份原普通副本）] → 符合预期：优先级区一份 + 普通待播原副本一份，非优先区副本不折叠；issue 5 只要求「下一首播放不新增重复」，不要求全局折叠。
- [Temporary 项经 `insert_next` 去重时用路径判等，路径可能因软链接语义不完全等价] → 临时项这一路径只来自文件打开命令，路径即身份，接受该语义并在 plan 中注明。
- [恢复的旧会话可能物理不连续] → 恢复不做物理重排，仅以 priority ID 为准；首次新的 `insert_next` 会重建连续切片。

## Migration Plan

1. 新增 `Queue::promote_to_priority`/修正 `insert_next`，在 `queue.rs` 内维护优先区不变量。
2. `coordinator.play_next` 调用去重路径；`queue_command.playNext` 分支继续构造 `lib_entry(id)`（参数不变）。
3. 增加队列单元测试（去重、提升、物理连续、普通待播不动、`clear_pending`/`remove` 后不变量）、协调器行为测试、前端类型/组件验证。
4. 验证沿用 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test` 及前端类型检查；对比归档 `refine-playback-queue-semantics/design.md` 的 FIFO 语义保持一致。回退策略：本 change 只扩展 `playNext` 路径语义，`enqueue`/普通入队行为不变，回退时直接还原 `insert_next` 实现即可，优先区持久化字段无需迁移。