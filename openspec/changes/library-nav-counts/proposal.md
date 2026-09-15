## Why

资料库导航（`资料库` 分组下的「全部歌曲 / 最近添加 / 喜欢的音乐」）在侧边栏打印每首视图的歌曲数量，但这个数字目前**寄生在内容区的分页查询结果上**：`publishLibraryCount()` 只在某个视图被打开并加载完最后一页时才被调用（`LibraryWorkspace.tsx:92-96`，`coverPalette.ts`）。

结果是用户必须先点进「喜欢的音乐」才能知道里面有多少首歌——而这恰恰是导航计数存在的意义（在点进去之前决定要不要点）。附带三个衍生缺陷：

1. **「最近添加」永远没有计数**：`App.tsx:148` 调用 `navItem("recent", ...)` 时压根没传 `countView`。
2. **超过一页就失准**：分页 `limit=200`，`isLast` 要滚到底才为 `true`，在那之前计数根本不发布；`favorites` 超过 200 首时数字要么不出现、要么反映的只是已加载页数。
3. **计数永不过期**：`clearLibraryCounts()` 定义了但全项目零调用。在「全部歌曲」里点红心、导入新歌、删除歌曲之后，侧边栏的「喜欢的音乐」计数都不更新——因为刷新它的 effect 只在 favorites 视图自己挂载时才跑。

根因是架构性的：IPC 层**没有独立的计数查询**（`commands.rs` 只有 `all_songs / favorites / recent / playlists`），侧边栏只能等内容区"顺便"告诉它。而 `PlaylistView.memberCount` 证明这条路是走得通的——歌单计数由后端 SQL 直接给出，只有资料库三个视图没有。

## What Changes

- **Core（`echo-core`）**：`domain/catalog.rs` 新增 `CatalogCounts { all, favorites, recent }`；`CatalogQueryRepository` 新增 `counts()`；`SqliteDatabase` 用两条 `COUNT(*)` 实现（active root + `availability = 'available'`，favorites 追加 `is_favorite = 1`，`recent = min(all, 100)` 以符合「最近添加」的最多 100 首定义）；`MemoryDatabase` 实现同语义；`CatalogQuery` 用例暴露 `counts()`。
- **Desktop（`echo-desktop`）**：新增 `LibraryCountsDto`（camelCase: `all` / `favorites` / `recent`）与 `AppServices::library_counts()`；`ipc/generate.rs` 追加生成类型；新增 Tauri 命令 `library_counts` 并在 `main.rs` 注册。
- **前端**：`bridge` 的 `BridgeCommandMap` 新增 `library_counts`；`coverPalette.ts` 的计数 store 从"被动接收发布"改为由新的 `useLibraryCounts` hook 主动拉取；订阅 `library://status` 与前端 `songUpdates` 广播作为失效信号重取；`publishLibraryCount` 退化为乐观覆盖（在本地已知变更时立即 +/-1，避免等待一次往返）。
- **导航渲染**：`App.tsx` 为「最近添加」补上计数，并沿用同一数据源为歌单项显示 `memberCount`。

## Capabilities

### New Capabilities

（无。本 change 不引入新能力，全部落在既有 `library-experience` 与 `desktop-app-shell`。）

### Modified Capabilities

- `library-experience`: 新增「资料库视图计数」需求——三个视图的计数必须由后端独立查询给出，在导航可见时即已确定，不依赖用户是否打开过该视图。
- `desktop-app-shell`: 新增导航计数的可观察行为——资料库三个视图与歌单均在侧边栏显示计数。

## Impact

- **代码**：`crates/echo-core/src/domain/catalog.rs`、`application/ports.rs`、`application/catalog.rs`、`infrastructure/sqlite/{mod,query}.rs`、`application/testing/memory_database.rs`；`crates/echo-desktop/src/ipc/{dto,generate}.rs`、`runtime/services.rs`、`apps/desktop/src-tauri/src/{commands,main}.rs`、`apps/desktop/src/ipc/ipc-types.generated.ts`（生成）、`apps/desktop/src/bridge/index.ts`、`features/library/coverPalette.ts`、`app/App.tsx`。
- **接口**：新增 Tauri command `library_counts`（纯新增，不改动既有命令签名或返回结构）；IPC DTO 新增一个类型，属向后兼容的增量。
- **不改变**：现有 `all_songs / favorites / recent / playlists` 命令、游标分页语义、`PlaylistView` 结构、capability 权限集、CSP。
- **架构约束**：计数规则（active root、pending-delete 隐藏、「最近添加」上限 100）全部留在 Core 的 SQL/领域层，桌面层只做 DTO 映射；UI 不得自行从已加载页数推算总数。
- **验证**：Core 单测覆盖两个仓库实现（SQLite + 内存）的语义一致；前端组件测试断言「未打开过的视图也显示计数」；`pnpm verify:task` 回归既有任务。
