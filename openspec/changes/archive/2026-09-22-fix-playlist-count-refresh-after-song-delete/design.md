## Context

见 `proposal.md`。桌面壳层通过 `useLibraryPlaylists` 读取包含 `memberCount` 的歌单导航快照；歌曲菜单的 `onRefresh` 回调由当前视图提供。删除成功和撤销成功已经会刷新当前视图以及资料库总数，但歌单视图和普通曲库视图传入的回调只覆盖了当前列表。

## Goals / Non-Goals

**Goals:**

- 让单曲删除与撤销在所有发起视图中都触发歌单导航列表的权威重读。
- 保持计数只在后端操作成功后刷新，失败路径不改变当前歌单快照。
- 通过回归测试覆盖普通曲库视图和歌单视图的刷新协调。

**Non-Goals:**

- 不改变删除、撤销、级联移除或 SQLite 持久化语义。
- 不新增 IPC 命令、事件或前端缓存层。
- 不把歌单计数改为基于当前已加载歌曲列表的乐观计算。

## Decisions

### 1. 在视图边界组合“本地刷新 + 导航刷新”

让 `LibraryWorkspace` 和 `PlaylistsView` 为 `SongMenu.onRefresh` 提供各自的组合回调：先刷新当前歌曲集合，再调用已有的 `onLibraryChanged`。这样刷新责任仍由拥有查询的视图协调，`SongMenu` 不需要知道歌单导航或 App shell。

备选方案是让 `SongMenu` 直接导入并触发歌单 store；该方案会跨越 feature 边界、让通用歌曲菜单依赖 shell 状态，也无法表达当前视图自己的查询刷新，因此不采用。

### 2. 撤销复用同一成功回调

`SongMenu` 已经只在 `delete_song` 或 `undo_delete` Promise 成功后调用 `onRefresh`。组合回调同时用于删除和撤销，可保证两条路径的计数行为一致，并保持失败时不刷新。

### 3. 保持后端快照为权威来源

不根据删除歌曲所在歌单数量做前端 `-1` 计算。每次成功 mutation 触发已有 `useLibraryPlaylists` 的 `playlists` 查询，从后端返回真实的 `memberCount`；这也覆盖同一歌曲属于多个歌单和部分级联清理等情况。

## Risks / Trade-offs

- [Risk] 删除一首歌曲会额外发起一次歌单列表查询。→ 这是已有导航刷新抽象的最小使用方式，查询范围很小且避免了多歌单前端推断错误。
- [Risk] 删除撤销与导航重读存在异步竞态。→ `useLibraryPlaylists` 已按 root 和取消标记处理迟到结果；回调只在后端 Promise 成功后触发，保持当前请求顺序语义。

## Migration Plan

无需数据迁移或兼容处理。发布桌面前端后，后续删除/撤销操作会使用新的组合刷新回调；已有陈旧 UI 只需重新打开应用或触发一次现有歌单刷新即可恢复。
