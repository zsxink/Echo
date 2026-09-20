## 1. Core 候选歌曲查询

- [x] 1.1 在 `echo-core` 的资料库查询边界增加“当前活动根目录中最新可用歌曲”的查询能力，按 `added_at` 降序复用既有稳定 tie-break，并在 SQLite/内存查询实现中只排除非活动根、缺失和 pending-delete 歌曲；用 Core 单元测试验证最新时间、相同时间 tie-break、无活动根和无可用歌曲场景
- [x] 1.2 运行 `cargo test -p echo-core`，确认候选查询、`added_at` 排序和既有资料库恢复测试全部通过

## 2. 桌面播放会话的初始项编排

- [x] 2.1 在 `echo-desktop` 运行时服务中增加可复用的“无有效 current 时选择候选”编排，先使用现有恢复裁决移除失效 current，再调用 Core 查询；验证有效 current、blocked 项、跨根歌曲和空结果不会误触发兜底
- [x] 2.2 通过现有暂停态队列/prime 路径把候选歌曲建立为唯一当前 entry，继承 mode、volume、muted 等设置且不调用播放器播放；在 `PlaybackCoordinator`/会话测试中验证队列投影、当前项、暂停状态和无 `play` 调用
- [x] 2.3 将启动恢复、资料库重建完成和外部删除当前歌曲的流程接入该编排，保持有效会话的 current、队列、位置和历史语义不变；运行 `cargo test -p echo-desktop` 验证重装/新设备、外部删除、空库和重复恢复场景

## 3. 首次导入后的默认选中状态

- [x] 3.1 在导入提交成功后的桌面状态通知/刷新边界调用同一初始项编排，仅当导入前后没有有效 current 且确实产生第一首资料库歌曲时设置它；验证导入失败、重复导入和已有播放会话不会替换 current 或自动播放
- [x] 3.2 将初始项状态通过既有类型化 IPC/事件同步到常驻播放栏和曲库选择状态，确保封面、标题、艺人等可用字段显示而不直接从 UI 拼装播放队列；运行 `pnpm --dir apps/desktop test` 与 `pnpm --dir apps/desktop typecheck`

## 4. 回归与交付验证

- [x] 4.1 补齐跨层场景测试，覆盖空资料库、首次成功导入、首次导入失败、重开已有资料库、上次歌曲被外部删除、所有歌曲不可用、有效 current 优先恢复以及不自动播放；执行对应 Rust 和桌面测试命令并保存通过结果
- [x] 4.2 执行 `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop format:check`、`pnpm --dir apps/desktop build`，修复本变更引入的格式、静态检查、类型或构建问题
- [x] 4.3 执行 `openspec validate --strict`，核对两份增量规格与实现测试场景一致，确认没有新增数据库/播放会话迁移或绕过 Core/播放器分层的实现残留
