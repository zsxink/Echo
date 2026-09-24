## Why

资料库工作区存在三个割裂的界面缺口：歌单视图没有搜索框也搜不了当前歌单；大量曲库或长歌单下找不到当前正在播放的歌曲在哪一行；切换到歌单、歌手或专辑视图后，品牌区的导入入口消失。这些缺口让「搜索→定位→入库」的日常闭环在部分视图断掉。

## What Changes

- 歌单详情视图增加搜索框，支持在**当前歌单内**按歌曲名、艺人和专辑做包含搜索，并沿用现有分页、排序与空状态；搜索与歌单成员读取由两条独立链路合并到同一条分页链路。
- 全部歌曲、最近添加、喜欢的音乐、歌单、歌手/专辑聚合详情页的资料库工具栏新增**定位到当前播放歌曲**按钮：当前视图包含正在播放歌曲时滚到列表可见区域顶部，不包含或未在播放时给出 Toast 提示，并正确处理分页未加载、目标位于列表末尾等边界。
- 导入入口从「仅全部歌曲/最近添加/喜欢的音乐视图由 `LibraryWorkspace` portal 填充」改为**由 App 壳层统一渲染到侧边栏品牌区**，使歌单、歌手、专辑视图下导入始终可用；行为与既有的 `choose_and_import_files` 导入流程一致，只读资料库仍禁用。

## Capabilities

### New Capabilities

### Modified Capabilities
- `library-experience`: 资料库搜索要求扩展到歌单范围（搜索当前歌单）；资料库工具栏新增定位当前播放歌曲控件及其边界行为。
- `playlist-management`: 歌单查看要求增加「歌单内搜索」场景，歌单详情视图提供搜索框并限定在当前歌单范围。
- `artist-album-browsing`: 歌手/专辑聚合详情歌曲页的资料库工具栏新增定位当前播放歌曲控件。
- `desktop-app-shell`: 明确品牌区的设置与导入操作在所有资料库视图（含歌单、歌手、专辑详情）下始终可用，导入入口不再依赖当前路由视图。

## Impact

- **Rust Core**：`CatalogQueryRepository::search`（`crates/echo-core/src/application/ports/repository.rs`）扩展可选歌单范围；`SqliteDatabase`（`crates/echo-core/src/infrastructure/sqlite/query.rs`、`mod.rs`）在 `query_active` 增加 `JOIN playlist_songs` 谓词并保持键集分页；`CatalogQuery::search`（`crates/echo-core/src/application/catalog.rs`）及内存测试替身（`application/testing/memory_database/catalog.rs`、`PagedCatalog`）同步。
- **echo-desktop**：`AppServices::search`、Tauri command `search` 接受可选歌单 ID；`PlaybackContextRequest::view = ViewRef::Playlist` 的 `resolve_library` 补齐歌单+查询分支（当前 `unreachable!`）。
- **桌面前端**：`useSongs` 与 `PlaylistsView` 接线歌单内搜索；`SongList` 增加定位 prop 与滚动定位；三视图工具栏插入定位按钮；`Icon` 新增定位图标；导入逻辑上移到 App 壳层共享 hook。
- **类型生成**：IPC DTO 与生成式 TypeScript 类型（`apps/desktop/src/ipc/ipc-types.generated.ts`）随命令参数变化重新生成。

非目标：本期不实现歌单内手动排序；定位按钮不做「切换到含该歌曲的视图」跨视图跳转；不新增账号/榜单能力。