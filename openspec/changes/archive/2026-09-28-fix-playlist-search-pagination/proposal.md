## Why

歌单内搜索把结果静默截断在 200 条：请求固定 `cursor: null, limit: 200`，传给歌曲列表的 `onLoadMore` 是空实现，超出第一页的命中结果没有任何途径加载。标题计数却读后端全量命中数，于是命中超过 200 时界面自相矛盾——标题写着「1200 首」，列表到第 200 行就到底，用户无法察觉已被截断。

这直接违反既有规格：`library-experience` 的「歌单内搜索」要求搜索结果与成员列表共用相同的**分页/排序规则**，`playlist-management` 的「歌单详情搜索」再次引用该分页约束，而同一能力下的「分页歌曲列表显示匹配总数」要求总数不受已加载页数影响。总数正确、列表跟不上，两个需求在当前实现下无法同时成立。

## What Changes

- 歌单内搜索结果改为键集分页：接入后端 `search` 已有的 `cursor`/`nextCursor`/`isLast` 契约，用滚动触底续页，命中数超过单页大小时可加载后续分页。
- 搜索词变化、排序变化、切换歌单时重置游标与已加载分页，旧游标的迟到结果不得写入界面；并发请求只有最新一次可以落地。
- 末页（`isLast`）后不再触发续页，避免空转请求。
- 搜索态的计数与已加载范围保持与资料库视图一致的语义：标题显示完整命中总数，列表按已加载分页呈现。
- 成员列表（非搜索态）保持既有全量返回语义，不引入分页；`onLoadMore` 由「按态分发」取代空实现。
- 补齐组件测试覆盖跨页加载、末页终止、请求竞态失效三种边界，并同步场景三方对账（traceability / manifest / scenario 文件）。

非目标：不改后端分页语义与 `PagedSongs` 契约；不改成员列表的 `playlist_members` 全量路径；不做跨歌单的搜索结果合并；不改歌单定位（`useLocateSong`）在搜索态的行为。

## Capabilities

### New Capabilities

无。

### Modified Capabilities
- `library-experience`: 「歌单内搜索」要求补齐分页场景——搜索结果跨页续页、末页终止、条件变化重置与竞态失效，与资料库搜索遵循同一套键集分页规则。
- `playlist-management`: 「歌单详情搜索」要求补齐分页场景——歌单内搜索结果可加载后续分页，末页后不再续页。

## Impact

- **桌面前端（Presentation）**：`PlaylistsView.tsx` 的 `playlistsMemberSearch` 改为保存 `nextCursor`/`isLast` 的分页状态机并实现真实 `onLoadMore`；`PlaylistsView.test.tsx` 新增分页/竞态用例。
- **规格（spec）**：`library-experience` 与 `playlist-management` 的 delta spec 新增场景；现有 requirement 文本（「共用相同的可见范围与分页/排序规则」）保持不变，本次是把实现补齐到既有规格，而非新增规格。
- **可追溯性资产**：`docs/traceability.md` 与 `scripts/verify/manifest.json` 需与新增场景同步（由 `gen-scenario-manifests.mjs` 重生成）。

未触及 Rust Core、SQLite 查询、Tauri command 契约与生成式 IPC 类型——后端分页能力齐备，缺口纯在前端接线。
