## 1. 队列状态机与持久化

- [x] 1.1 在 `crates/echo-desktop/src/player/queue.rs` 为规范化循环投影、列表循环前后导航和 FIFO“下一首播放”插入区建立 entry-ID 不变量；为重复歌曲、回绕、连续插入、清空、blocked/failed/已删除 entry 添加单元测试，并运行 `cargo test -p echo-desktop player::queue`。
- [x] 1.2 在 `crates/echo-desktop/src/player/coordinator.rs` 以显式事件区分自然结束、手动上一首、手动下一首和错误跳过，落实三种模式及优先区消费规则；使用确定性随机源覆盖完整模式矩阵，并运行 `cargo test -p echo-desktop player::coordinator`。
- [x] 1.3 升级 `crates/echo-desktop/src/player/session.rs` 的版本化会话以保存/恢复优先区，并兼容旧会话为空优先区、临时项过滤和暂停恢复；验证新视图上下文替换不会混入持久化或旧会话的待播项，并运行 `cargo test -p echo-desktop player::session`。

## 2. 桌面边界与队列快照

- [x] 2.1 更新播放上下文入口、`apps/desktop/src-tauri/src/commands.rs` 与播放器事件发布路径：从任意视图点击歌曲时由桌面端解析完整视图、原子替换旧队列和手动优先区，并移除或限制可接收前端部分歌曲列表的公共路径；`player_control`、`queue_command` 只传递类型化意图并由协调器决定导航。补充歌单、搜索、收藏和曲库视图的 command/快照集成测试，证明分页/部分列表不会缩短队列后运行 `cargo test -p echo-app`。
- [x] 2.2 扩展播放器快照和生成的 IPC DTO，使每个队列 entry 包含标题、艺人、时长和封面引用（或明确空值），并在队列变更时批量/缓存解析资料库元数据；重新生成 IPC 类型并运行 `pnpm --filter @echo/desktop generate:ipc`。
- [x] 2.3 验证高频播放位置事件复用已解析的队列 DTO、不会触发按行 N+1 元数据查询；为队列变更与临时项兜底添加桌面测试，并运行 `cargo test -p echo-desktop`。

## 3. 播放队列面板

- [x] 3.1 更新 `apps/desktop/src/player/playerStore.ts` 和相关类型，严格消费扩展后的权威快照；运行 `pnpm --filter @echo/desktop typecheck`。
- [x] 3.2 更新 `apps/desktop/src/features/player/QueuePanel.tsx`，显示封面、标题、艺人和时长的兜底状态，并且只渲染服务端给出的规范化顺序；补充组件测试后运行 `pnpm --filter @echo/desktop test -- QueuePanel`。
- [x] 3.3 将普通模式现有的队列面板宽度、歌曲行高度、封面尺寸和最多 8 行可视区域提取为共享尺寸 token，并让沉浸模式复用这些值；超出时在列表区域内滚动且标题与操作固定可用。验证两个模式下尺寸相同、1–8 行无多余滚动、9 行以上可访问全部条目，并运行 `pnpm --filter @echo/desktop lint` 与 `pnpm --filter @echo/desktop format:check`。

## 4. 端到端回归验证

- [x] 4.1 新增覆盖列表循环回绕的上一首/下一首（上一首不消费优先区）、随机轮次、单曲循环自然结束及手动下一首、连续“下一首播放”FIFO、清空优先区、从歌单/搜索/收藏/曲库视图点击歌曲时以桌面端完整视图数量重建队列、恢复旧会话和当前项置顶的端到端或跨层回归场景；运行 `cargo test --workspace`。
- [x] 4.2 执行桌面前端完整验证：`pnpm --filter @echo/desktop typecheck && pnpm --filter @echo/desktop lint && pnpm --filter @echo/desktop format:check && pnpm --filter @echo/desktop test && pnpm --filter @echo/desktop build`。
- [ ] 4.3 构建后执行 `pnpm --filter @echo/desktop test:e2e`，手动确认队列有歌曲信息、初始最多显示 8 首且可滚动、当前歌曲始终在顶部、普通与沉浸模式尺寸一致，并记录任何平台播放器冒烟差异。
