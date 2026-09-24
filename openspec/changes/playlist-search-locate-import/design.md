## Context

现状（见 proposal.md - Why，实现现状经代码探查确认）：

- 搜索与歌单读取是两条独立链路：`search` 只接受 `query + in_favorites`，SQL 只 JOIN `songs`；歌单成员 `playlist_songs` 全量读取、无过滤、无键集分页。`ViewRef::Playlist` 在播放上下文 `resolve_library` 内是 `unreachable!("library view only")`。
- 歌曲列表是「键集分页 + 虚拟列表」：`SongList` 每页 200 条，只渲染视口窗口，滚动容器 `.table-wrap` 的 ref 是组件私有；行有 `data-song-id` 但虚拟列表下目标行可能未渲染，定位必须按 `index * ROW_HEIGHT` 数值计算。
- 导入按钮的渲染责任在 `LibraryWorkspace`（portal 或内联），而 `brand-import-slot` 槽位属于 App 侧边栏，只传给 `LibraryWorkspace`；`PlaylistsView`/`CollectionDirectory` 不接收 `importTarget`，故切到歌单/歌手/专辑视图时槽位为空。
- 工具栏 `.library-tools` 三视图结构一致（排序控件 + 多选按钮）；`Icon.tsx` 无定位语义图标；`notify()` Toast 全局可用。

## Goals / Non-Goals

**Goals:**
- 让「歌单内搜索」「定位当前播放歌曲」「导入全视图可用」三项以最小跨层面落地，保持一致的分页、排序与空状态行为。
- 歌单内搜索复用既有搜索的字段匹配、键集分页与 FTS/LIKE 组合，不另造一套查询。
- 定位基于列表索引的数值滚动，不依赖 DOM 查询，兼容虚拟列表与未加载分页。
- 导入统一在 App 壳层渲染，职责从视图组件移出。

**Non-Goals:**
- 不实现歌单内手动拖拽排序或其他重排入口。
- 定位按钮不做「跳转到另一个含当前歌曲的视图」的跨视图导航，只在当前视图内定位，找不到即提示。
- 不改变导入流程本身（仍走 `choose_and_import_files`）、不新增账号或榜单能力。

## Decisions

### D1：歌单搜索复用 `query_active`，新增 playlist 谓词

`SqliteDatabase::search` 委托的 `query_active` 已有「active root + availability + is_favorite + query(LIKE/FTS)」的谓词组合与键集分页。歌单搜索在 `clauses` 增加 `EXISTS (SELECT 1 FROM playlist_songs ps WHERE ps.song_uuid = s.uuid AND ps.playlist_uuid = ?)`（或 `JOIN playlist_songs`）并追加歌单 UUID 绑定值即可，FTS/LIKE 与分页完全复用。

**替代方案**：新建独立 SQL 函数 `query_active_in_playlist`。否决——会复制谓词组合与键集逻辑，改动面更大。

### D2：歌单搜索可见范围为「歌单可见成员」，而非仅 available

现有歌单成员查询用 `availability <> 'pending_delete'`（保留失效/缺失歌曲），`query_active` 用 `availability = 'available'`。歌单视图显示的东西应可被搜索到——若搜索仅限 available，会出现「歌单里有这首歌但搜不到」的割裂。

**决策**：歌单搜索走一条显式谓词——`s.availability <> 'pending_delete'`，与歌单成员查询的可见范围一致。实现上给 `query_active` 增加一个「范围模式」参数（`enum` 而非布尔堆叠，遵守 CODE_STANDARDS §4.2），区分 all/favorites/playlist 三种可见性谓词组合。

**权衡**：这使同一 `query_active` 内 availability 谓词按范围模式变化；`all_songs`/`favorites` 仍用 `available`，playlist 用 `<> pending_delete`。用 enum 表达，避免布尔参数堆叠。

### D3：port 扩展 `search` 签名加 `playlist: Option<PlaylistId>`，而非新增方法

`CatalogQueryRepository::search` 加 `playlist: Option<PlaylistId>` 参数，`None` 时表现与现状完全一致；`Some(id)` 时按 D2 走歌单范围。选择扩展签名而非新增方法：搜索的所有组合维度（sort/cursor/limit/query）在两种范围下通用，单一方法是复用 keyset 分页和调用链的唯一实现。

连带改动：`CatalogQuery::search`、`AppServices::search`、Tauri command `search`、内存替身 `MemoryDatabase`/`PagedCatalog`、`useSongs` 的 `fetchPage` 分发。播放上下文仅 `ViewRef::Playlist` 分支涉及（见 D4）。

**兼容性**：`search` command 的请求参数新增可选 `playlistId`，旧调用（`None`)行为不变；生成式 IPC 类型随命令参数变化重新生成。

### D4：播放上下文补齐歌单+query 分支，搜索后播放队列与筛选列表一致

`resolve_library` 对 `ViewRef::Playlist` 当前 `unreachable!`，而 `ViewRef` 已有 `Playlist { id }` 变体。为使「歌单内搜索 → 点击播放」的队列与用户看到的筛选后列表一致（与 `artist-album-browsing` 既有「详情页搜索词存在时播放队列与筛选后的歌曲列表一致」原则对齐），本轮把该分支补齐：`ViewRef::Playlist` 带 query 时走歌单搜索（D1/D2 链路），空 query 时保持现有 `resolve_playlist` 全量路径并按既有 newest-first 可见顺序。

**决策**：不做「播放上下文大改」。在 `resolve_library` 增加 `Playlist` 分支、复用既有的逐页拼装（`PAGE_SIZE = 500`），使歌单搜索也可分页解析。`play_playlist_context` command 增加可选搜索词参数（沿用前端 SongQuery 已预留的 `playlistId` 语义）。

### D5：定位采用「数值滚动」，SongList 增加 `locateSongId` prop

`SongList` 是键集分页 + 虚拟列表，行 `data-song-id` 只在已渲染窗口内存在。定位不做 `querySelector`，而是在 `SongList` 增加 prop：

- `locateSongId?: string | null` + 内部 `useEffect`（依赖 `locateSongId`、`songs`（实际传给列表的排序后数组）、`scrollTop`、`loading`）执行滚动。
- 目标在 `songs` 内 → `viewportRef.current.scrollTop = index * ROW_HEIGHT`（置顶）；若 `index * ROW_HEIGHT` 超出当前可滚范围（末尾边界，`index * ROW_HEIGHT + ROW_HEIGHT > scrollHeight - clientHeight`），取 `max(0, scrollHeight - clientHeight)` 对齐到最后的实际可行位置。
- 目标不在 `songs` 且 `!isLast && !loading` → 触发 `onLoadMore()` 并进入「等待中」；每次 `songs` 变化时重新 `findIndex`，找到后再滚动（异步分页，时机由 next-frame effect 保证）。
- `currentSongId === null`（未在播放）或 `findIndex === -1` 且 `isLast` → `notify()` 提示，不改变滚动。

**替代方案**：在视图层暴露 `viewportRef` 并自行算滚动。否决——定位与列表自身分页/虚拟窗口耦合，放 `SongList` 内聚更高。图标：在 `Icon.tsx` `GLYPHS` 新增一个定位字形（现有无可复用者；fallback 会退回 `library`，不可接受）。

### D6：导入上移为 App 壳层共享 hook，直接在品牌区渲染

把 `LibraryWorkspace` 中的 `runImport` / `importing` / `importFailures` 抽出为共享 hook（`useImport`，含 `ImportFailureDialog` 渲染），由 `App.tsx` 在侧边栏 `brand-actions` 直接渲染导入按钮（与设置按钮平级）。`brand-import-slot` 保留为样式锚点但内容改由 App 填充；`PlaylistsView`/`CollectionDirectory` 不接收 `importTarget`。

- 删掉 `LibraryWorkspace` 的 `importTarget === undefined` 内联分支（isolated workspace 测试依赖的「导入中…」/「导入」按钮形态随之消失，相关 UI 测试同步到壳层测试）。
- `readOnly` 已在 App 作用域可用（`status.readOnly`），直接决定禁用/隐藏，符合后端 `guard_writes` 双重把关。
- `runImport` 里的 `invalidateLibrary()`/`onLibraryChanged` 依赖由 hook 以 props 传入，保持导入后计数/列表刷新语义不变。

**替代方案**：继续往各视图传 `importTarget` 让每个视图自己 portal。否决——会把同一份导入逻辑复制到三个视图，维持「渲染责任在视图、槽位在壳」的脆弱结构。

## Risks / Trade-offs

- [歌单搜索可见范围与 `all_songs` 不同] → 范围差异由 D2 的 enum 显式承载，行为以歌单视图可见成员为准，避免与成员列表割裂；spec 场景「在歌单内搜索」已声明可见范围一致。
- [给 `search` 签名加参数影响所有调用点] → 是预期内的连带改动（D3）；`None` 保持现状，rust 测试可通过既有用例回归；IPC 类型重新生成。
- [定位等待未加载分页是异步时序，可能抖动] → 用 next-frame effect + 在 `songs` 变化时重算；测试覆盖「目标在后续页」与「目标不存在」两态。
- [导入上移改变组件结构，isolated-workspace 测试崩溃] → tasks 列为显式步骤：迁移主页回壳层渲染路径并更新容器测试，不让旧测试阻碍。
- [歌单搜索后播放依赖 D4 的上下文分支] → 若 D4 被判定超出范围，则「搜索后点击播放按筛选结果」退化为按全歌单播放；tasks 以 D4 为一个独立任务，便于单独取舍。

## Migration Plan

- IPC contract 变更（`search` 加可选 `playlistId`）与前端生成类型同步生成，无破坏性默认值；无持久化 schema 变更。
- 导入渲染迁移是纯前端结构重组，无数据迁移；`brand-import-slot` 样式保留。
- 分阶段提交，避免一次 PR 混合三层：Core+端口 → desktop+IPC → 前端视图。

## Open Questions

无。