# 合成基线与恢复反馈验证

验证日期：2026-10-23。本文件只记录实际运行的检查，不代表整个 change 已完成。

## issue #72 合成基线（1.2 / 1.3）

`application/testing/library_durability.rs` 提供可复用 `Issue72Fixture`：15 个合成媒体、2 条 portable Song、11 条 Favorite、3 条 PlaylistItem、0 条 portable Playlist。已证明版本另带完整逻辑数据库 after-images 和明确 Song alias；孤立版本没有这些救援证据。两者 portable 资料完全相同，身份与字节均为固定合成数据。

- `cargo test -p echo-core library_durability_issue72 --lib`：2 条测试通过；验证两种证据来源和旧 continuation 只能恢复 2 首、14 条关系 unresolved，原资料不被删除。
- `cargo test -p echo-core library_durability_comparator --lib`：1 条测试通过。

`CanonicalLibraryState` 用稳定 typed object key 比对完整 envelope、私有 state 与资源声明。比较包含 UUID、路径/hash/added_at、显式 favorite false、空歌单、成员 UUID/关联/位置/时间、覆盖/歌词、封面原件、逐设备桶、typed tombstone、alias 与冻结 pending 前驱；改变数据但保持行数的反例能检出。`from_replay` 同时接受 checkpoint 和有序后缀，供 wipe/压缩/多设备验收复用。此处测试只证明基线和比较器，历史迁移与完整恢复仍待后续任务验证。

## IPC 错误与同操作恢复（9.5，9.1 / 9.2 部分）

新错误 `library_saved_pending_recovery` / `library_saved_pending_projection` 携带原 operation ID。前端将两种 code 和 message key 翻译为“已保存”中文提示；显式 `ErrorPolicy` 表均禁止盲目重新发起原创建/编辑。恢复入口 `retry_library_operation` 验证预期 active root，读取原 operation、恢复目录并追赶完整投影，不再执行原导入。

- `cargo test -p echo-core library_durability_retry --lib`：3 条测试通过（未知操作拒绝、完整有序追赶且不新增提交、投影失败保留原身份）。prepared-only 拒绝由目录协议 FS 测试覆盖。
- `cargo test -p echo-desktop ipc::error --lib`：4 条测试通过，包含新码、重试策略、operation ID、序列化无内部原因泄露。
- `cargo run -p echo-desktop --bin echo-generate-ipc`：成功生成类型。
- `cargo test -p echo-desktop ipc::generate --lib`：3 条通过，包含生成物与生成器一致性。
- `pnpm --filter @echo/desktop typecheck`：通过。
- `pnpm --filter @echo/desktop exec vitest run src/features/library/libraryImportRefresh.test.tsx src/features/import/ImportBatchDialog.test.tsx src/features/import/importFeedback.test.ts src/bridge/libraryPersistence.test.ts`：4 个文件、18 条通过。覆盖混合批次、原库/原操作恢复、不重导、恢复失败保留已保存资料。
- 针对已改前端文件的 ESLint：通过。

9.1 / 9.2 尚未整体完成：设置页持久恢复报告、所有喜欢/歌单/播放入口及重新打开反馈仍需接入，不以现有导入测试代替。
