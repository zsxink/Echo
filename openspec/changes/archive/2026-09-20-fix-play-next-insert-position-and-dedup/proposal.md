## Why

「下一首播放」当前不去重：从曲库/歌单进入完整视图队列后，对同一首歌反复点「下一首播放」，每次都会新建独立 queue entry（`insert_next` 直接用插入前的 `priority.len()` 计算偏移，物理插到优先区末尾），导致同一首歌在队列中堆积多份、普通待播项被不断往后挤——这正是 issue 5 观察到的「越插越靠后、重复 entry 堆积」观感。先前 change 已把「连续下一首播放」规格化为 FIFO 优先区（A→B→C 顺序消费），抽象语义正确；缺失的是**去重 + 提升**：歌曲已在队列中时应提升到优先区对应位置，而不是新增重复项。

## What Changes

- 将「下一首播放」从「总是新建 entry 追加到优先区末尾」改为**去重 + 提升**:待插入歌曲已在当前队列中（含已存在于优先区或普通待播项）时,不再新建 entry,而是把该歌曲**已存在的队列位置**提升到下一首优先区的最前头（紧邻当前项之后）;不在队列中时才作为新 entry 追加到优先区末尾。
  - 仅当插入来自「下一首播放」时才提升;「加入播放队列」仍允许同一首歌多次出现在队列中。
  - 重复判据在桌面层统一为 **QueueItem 身份**(歌曲 SongId 或临时项身份),不按 QueueEntryId 折叠,保持「同一歌曲可多次出现在队列」的既有不变量在非 playNext 路径不变。
- 修复 `Queue::insert_next` 的脆弱物理偏移:优先区物理上必须与当前项连续相邻(优先区自身 FIFO、紧贴当前项之后),不允许依赖「插入前 priority.len()」这一仅在局部成立的前提,避免会话恢复或移除后插入位置错位。
- 移除优先区已存在的重复项时,只删除**新增之外的那份**,当前项与其余待播项相对顺序保持不变;同一首歌若在优先区已多次存在,提升到最前头的那份成为唯一优先项,其余重复副本从优先区清除,普通待播区中该歌的别的副本保持不动(它们仍会按列表循环正常轮到)。
- 同步演进 Tauri `queue_command` 的 `playNext` 分支与前端调用方,使返回语义一致(新返回被提升/新建的 entry 身份),并补充队列单元测试、协调器行为测试与前端类型/组件验证。

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `desktop-playback`: 修改「下一首播放」「连续下一首播放」场景,加入去重 + 提升语义与优先区物理连续不变量。

## Impact

- 受影响代码位于桌面平台层:`crates/echo-desktop/src/player/queue.rs`(`insert_next`、`priority` 维护、新增提升路径)、`crates/echo-desktop/src/player/coordinator.rs`(`play_next`)、`apps/desktop/src-tauri/src/commands.rs`(`queue_command` 的 `playNext`)。播放队列仍严格留在 `echo-desktop`,不进入 `echo-core`。
- Tauri command 返回语义可能扩展(返回被提升/新建的 entry id),React 消费方与类型需同步;既有 `playNext` 传参(SongId)不变,因此不构成破坏性接口变更。
- 优先区持久化格式不变(仍按 QueueEntryId 列表持久化),但恢复与 `set_priority` 需容忍优先区不再物理连续的旧会话并修复之。
- 需同步检查 `docs/DESIGN.md` 播放队列章节与归档的 `refine-playback-queue-semantics` 设计是否一致;验证沿用 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test` 及前端类型检查。