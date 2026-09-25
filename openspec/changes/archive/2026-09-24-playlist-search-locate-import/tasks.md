## 1. Core：歌单内搜索

- [x] 1.1 在 `CatalogQueryRepository::search` 增加 `playlist: Option<PlaylistId>` 参数，`None` 保持现有范围语义；同步更新 `CatalogQuery::search`、内存替身 `MemoryDatabase` 与 `PagedCatalog` 测试替身，运行 `cargo test -p echo-core` 确认既有搜索测试通过且 `None` 行为无回归
- [x] 1.2 在 `query_active`（`crates/echo-core/src/infrastructure/sqlite/query.rs`）用范围模式 enum 表达 all/favorites/playlist 三种可见性谓词：playlist 模式追加 `EXISTS(SELECT 1 FROM playlist_songs ps WHERE ps.song_uuid = s.uuid AND ps.playlist_uuid = ?)`，availability 谓词为 `<> 'pending_delete'`（与歌单成员一致），保留 query 的 LIKE/FTS 组合与键集分页；新增 Core 集成测试覆盖「歌单内搜索命中/无命中、空搜索词恢复全量、失效成员可被搜索、分页游标正确」并通过
- [x] 1.3 补齐播放上下文 `resolve_library` 的 `ViewRef::Playlist` 分支（当前 `unreachable!`）：带 query 时走歌单搜索并按 `PAGE_SIZE` 逐页拼装，空 query 时保持既有 `resolve_playlist` 全量路径；新增测试覆盖「歌单内搜索后点击播放的队列与筛选后列表一致」并通过

## 2. desktop IPC：歌单搜索命令与类型

- [x] 2.1 `AppServices::search` 与 Tauri command `search` 接受可选 `playlistId`（parse 为 `Option<PlaylistId>`），`play_playlist_context` 接受可选搜索词并走新的 `ViewRef::Playlist` 分支；运行 `cargo test --workspace` 相关用例确认 IPC 契约无回归
- [x] 2.2 运行 IPC 类型生成（`pnpm --filter @echo/desktop generate:ipc` 或 `cargo run -p echo-desktop --bin echo-generate-ipc`）并运行 `pnpm --filter @echo/desktop typecheck`，确认生成式 `ipc-types.generated.ts` 与 bridge 参数类型一致、`AssertNever` 防漂移检查通过

## 3. 前端：歌单内搜索

- [x] 3.1 在 `useSongs` 的 `fetchPage` 分发歌单范围：`query.playlistId` 存在时（无论搜索词是否为空）统一调用带 `playlistId` 的 `search`，空搜索词由 `query_active` 的 playlist 分支恢复歌单完整成员并复用键集分页；利用已预留的 `SongQuery.playlistId` 与 `pageKey` 缓存键；更新 `useSongs` 单测覆盖歌单搜索分发、空词恢复与缓冲键区分
- [x] 3.2 `PlaylistsView` 的 `Topbar` 增加搜索输入（与资料库搜索控件一致的外观与占位），搜索状态驱动 `useSongs` 歌单范围查询；无结果时展示歌单视图的搜索空状态（沿用「搜索空状态」语义与清除操作）；运行 `pnpm --filter @echo/desktop test` 相关组件测试并手动验证歌单视图搜索、清空、无结果三态

## 4. 前端：定位当前播放歌曲

- [x] 4.1 在 `Icon.tsx` `GLYPHS` 新增定位字形（locate）并处理 fallback，运行 `pnpm --filter @echo/desktop lint` 确认无类型/规范问题
- [x] 4.2 在 `SongList` 增加 `locateSongId` prop 与内部 `useEffect`：目标在 `songs` 内按 `index * ROW_HEIGHT` 置顶滚动，越界时对齐到 `max(0, scrollHeight - clientHeight)`；目标缺失且 `!isLast && !loading` 时触发 `onLoadMore()` 并在 `songs` 变化后于下一帧重试；`currentSongId` 为空或不存在时调用方提示、不滚动；用组件测试覆盖置顶、末尾边界、未加载分页定位与不存在提示
- [x] 4.3 在 `LibraryWorkspace`、`PlaylistsView`、`CollectionDirectory`（聚合详情分支）的 `.library-tools` 于 `SongSortControl` 之后插入定位按钮，读取 `usePlayerSnapshot().currentSongId`，命中时设置 `locateSongId`，未命中时 `notify()` 提示「当前没有正在播放的歌曲」/「当前歌曲不在此列表中」；最近添加视图无排序控件也显示定位按钮；运行 `pnpm --filter @echo/desktop test` 覆盖三视图定位交互

## 5. 前端：导入入口全视图可用

- [x] 5.1 将 `LibraryWorkspace` 的 `runImport`/`importing`/`importFailures` 与 `ImportFailureDialog` 抽出为共享 `useImport` hook（导入结果反馈、`invalidateLibrary`/`onLibraryChanged` 语义通过 props 注入），保留 `choose_and_import_files` 既有行为
- [x] 5.2 由 `App.tsx` 在侧边栏 `brand-actions` 直接渲染导入按钮（与设置按钮平级，样式复用 `brand-action`），`readOnly` 时禁用/隐藏；移除 `LibraryWorkspace` 的 `importTarget` prop、两处渲染分支与 `importTarget === undefined` 内联形态，删除传给 `PlaylistsView`/`CollectionDirectory` 的无关 prop；更新原 `LibraryWorkspace` 容器测试到壳层/共享 hook 路径，运行 `pnpm --filter @echo/desktop test` 覆盖歌单、歌手、专辑视图导入可用且只读禁用

## 6. 最终复核

- [x] 6.1 执行 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`pnpm --filter @echo/desktop lint` 与 `typecheck`，全绿确认无格式/规范回归
- [x] 6.2 运行 `openspec validate --changes`、`pnpm verify:task -- --all`（受影响的已验证任务），并复核四个 delta spec 的 requirement 与实现一致（歌单搜索范围、定位边界、导入全视图可用）；确认 traceability 与场景清单同步