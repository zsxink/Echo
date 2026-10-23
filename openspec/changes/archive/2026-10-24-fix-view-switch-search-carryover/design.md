# Design

## 背景与根因

三处带搜索框的列表（`PlaylistsView`、`LibraryWorkspace`、`CollectionDirectory`）在 `App` 中各占一个固定树位置，视图切换只换 props、不重挂载实例；搜索词是组件本地 `useState`，生命周期绑在实例上而非「当前视图」上，于是同类切换（歌单 A → B、三歌曲视图互切、歌手 ↔ 专辑）后旧词继续过滤新视图。同一文件族里排序已用 `useStoredSongSort` 按 scope 派生解决了同类问题，搜索词缺少对应处理。

## 方案：并入既有「导航即重置」副作用

三个组件各自已有「切换即重置选中态/批量模式」的 effect（`PlaylistsView.tsx` 以 `playlistId`、`LibraryWorkspace.tsx` 以 `view`、`CollectionDirectory.tsx` 以 `kind`/`root` 为键），把搜索清空加进同一副作用，与既有重置共享同一时机与语义：

- `PlaylistsView`：`setSearchText("")`。派生态（`searched`/`searching`/`searchCursor`）不需要单独复位——`searchText` 变化触发既有的搜索 effect 走空词分支，与用户手动清空完全是同一条路径（`fix-playlist-search-pagination` 已为其建立竞态与游标语义）。
- `LibraryWorkspace`：`setSearch("")`。`search` 进入 `useSongs` 的 query key，空词触发重查，视图本身的 reqId 机制丢弃旧词迟到结果。
- `CollectionDirectory`：`setSearch("")`。目录与详情两个查询 effect 的 key 均含 `search`，重查自动发生，`directoryRequest`/`detailRequest` 代际计数丢弃迟到结果。

### 被否决的替代方案

- **给组件加 `key` 强制重挂载**：副作用过大，顺带清掉选中态、批量模式、菜单状态与滚动位置，而这些已各有精确 effect 处理（issue #47 明确不采用）。
- **按视图 scope 派生搜索词（对齐 `useStoredSongSort` 的 render 期派生）**：能省掉切换时的一次多余请求，但要把三个组件的搜索 state 改成 scope 包装结构，改动面与回归风险都大于收益；切换瞬间的列表本身在重查中，一次被丢弃的旧词请求用户不可见。
- **在搜索 effect 内侦测「歌单变了但词没变」再清空**：等价于 scope 派生但更绕，不如直接在导航重置副作用里清空直白。

## 已知取舍：切换时的一次被弃请求

清空发生在 effect 里，而三处的查询 effect 在同一次 commit 也会以「新视图 + 旧词」重跑一次（它们的 key 里视图与搜索词都有）。该请求发出后即被随后的空词重跑作废（`searchRequest`/reqId/`directoryRequest`），结果不会写入界面；代价是每次「带词切换」多一次 IPC 往返，相对按键即查的既有行为可忽略。选择这一取舍是为了保持三处改动各一行、与既有重置语义完全同构。

## 规格修正

`library-experience`「资料库视图」的「切换全部歌曲」场景原文「保留当前搜索与排序控件的明确状态」与本次产品决策冲突：其本意是控件状态不暧昧（排序有各自偏好），而非保留搜索词。修正为「排序按该视图已保存偏好；搜索词被清空」，并以两个新场景把清空行为固化为可验证要求（歌曲视图互切、歌手/专辑目录互切）；`playlist-management`「歌单详情搜索」追加「歌单切换清空搜索」场景。三个场景均为 requirement 末尾追加，`spec-scenarios.mjs` 的位置编号不会重排既有 ID（LE-R01-S05/S06、PM-R09-S05）。

## 影响边界

只动桌面前端三个组件与其测试；分层上属 Presentation 内部行为修正，不新增跨层依赖、不改 IPC 契约、不改 Core。测试就近放在被测组件旁（三个既有 `.test.tsx`），沿用各文件已有的 harness（`PlaylistsView` 的命令分发 bridge mock；`CollectionDirectory` 的 `__echoTest.setInvoke` + rerender 切 kind；`LibraryWorkspace` 渲染真实 `App` 经侧边栏切换视图）。
