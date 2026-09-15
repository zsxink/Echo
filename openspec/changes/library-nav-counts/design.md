## Context

侧边栏导航计数当前由前端的模块级 store（`coverPalette.ts`）持有，写入方只有一个：`LibraryWorkspace` 在某个视图翻到最后一页时调用 `publishLibraryCount()`。这是一个**把导航级信息推导自内容级状态**的结构——导航需要的是"这个视图有多少首"，而内容区只知道自己"已经加载了多少首"。两者在分页未满时并不相等。

改为后端独立计算总数后，导航不再依赖任何视图是否被打开过，也消除了 >200 首时的分页歧义。

## 决策

### D1. 计数在 Core 计算，不在桌面层拼装

`counts()` 落在 `CatalogQueryRepository`（与 `all_songs` / `favorites` / `recent_100` 同一个 port）。理由：三个视图的**成员定义**（active root、`availability = 'available'` 即 pending-delete 隐藏、recent 上限 100）本来就由 Core 独占（`catalog.rs` 模块 doc：`echo-core stays the sole authority for which songs a view contains`）。若让桌面层用 `favorites(sort, None, usize::MAX)` 之类的手段自己数，就会在 Core 之外复制一份视图语义，之后任一处改动都会漂移。

### D2. 一个 `counts()` 而不是三个独立方法

三个数共享同一组过滤条件（active root + available），一次调用在一个读连接上取完，避免三次往返和三次 root 解析。返回类型是领域值对象 `CatalogCounts`，不是三个 `usize`——避免布尔/裸数值参数堆叠（CODE_STANDARDS §4.2）。

### D3. `recent` 在 Core 侧截断为 100

「最近添加」的规格定义是**最多 100 首**。若返回 `all`，导航就会出现"最近添加 5000"这种与视图内容矛盾的数字。截断放在 Core，UI 不做 `Math.min`。

### D4. 前端：拉取 + 乐观覆盖，而不是纯拉取

纯拉取会让红心点击后计数延迟一个往返才变——用户点了却没反应，体感是坏了。因此保留 `publishLibraryCount` 作为**乐观覆盖**：本地已知 `favorite` 由 `false → true` 时立即 `+1`，`true → false` 时 `-1`；随后失效重取会用权威值校正。乐观值只影响 `favorites` 一项（`all` / `recent` 不受单次收藏切换影响，导入/删除走重取而非增量）。

失效信号有三个来源，全部收敛到 store 的一个 `revision` 计数器：

- `library://status` 事件（root 切换、扫描完成、可用性变化）——既有事件，不新增。
- 前端 `songUpdates` 广播（`set_favorite` 提交后的权威 `SongView`）——既有模块，不新增。
- 内容区显式调用 `invalidateLibraryCounts()`（导入、删除、歌单变更）——替代当前零调用的 `clearLibraryCounts()`。

### D5. `clearLibraryCounts` 更名为 `invalidateLibraryCounts`

原名暗示"清空数字"（那会让计数闪成空白再回来）。实际语义是"这份缓存不再可信，请重取"。改名以匹配行为：重取期间保留上一次的已知值，不闪烁。

### D6. 歌单计数复用既有 `memberCount`

`PlaylistView.memberCount` 已经由 `AppServices::playlists()` 给出（`PlaylistRepository::members(id)?.len()`），前端只是没渲染。本 change 只在 `App.tsx` 渲染它，不改后端。

## 数据流

```text
App (mount / revision 变更)
   │  useLibraryCounts()
   ▼
bridge.call("library_counts")
   ▼
Tauri command library_counts → AppServices::library_counts()
   ▼
CatalogQuery::counts() → CatalogQueryRepository::counts()
   ▼
SqliteDatabase: 2× COUNT(*) (active root, availability='available'[+is_favorite=1])
   ▼
LibraryCountsDto { all, favorites, recent } → store → 侧边栏渲染

失效：library://status 事件 / songUpdates 广播 / invalidateLibraryCounts()
   → revision++ → 重新拉取
```

## 分层与依赖方向

| 层 | 变更 | 依赖方向 |
|---|---|---|
| Domain | `CatalogCounts` 值对象 | 无外部依赖 |
| Application | `CatalogQueryRepository::counts()`、`CatalogQuery::counts()` | 依赖 Port，不依赖 SQLite |
| Infrastructure | `SqliteDatabase::counts()`、`MemoryDatabase::counts()` | 实现 Port，不定义业务规则 |
| Presentation（desktop） | `LibraryCountsDto`、`AppServices::library_counts()`、Tauri command | 依赖 Core，不执行 SQL |
| UI | bridge、`useLibraryCounts`、侧边栏渲染 | 只消费 DTO，不推算总数 |

无新增跨层依赖，无绕过既有抽象。所用模式：**Repository**（SQL 留在存储层）、**Ports and Adapters**（Core 不知道 Tauri 存在）、**Observer**（前端失效广播）。

## 兼容与迁移

- Tauri command 纯新增，既有命令签名与返回不变，无需迁移。
- 生成文件 `ipc-types.generated.ts` 由 `pnpm generate:ipc`（Rust 侧 `echo-generate-ipc`）更新，不手工编辑（CODE_STANDARDS §2）。
- `MemoryDatabase` 与 `SqliteDatabase` 必须语义一致：Core 既有测试用内存库，若两者对 pending-delete 或 inactive-root 的处理不同，测试会通过而生产是错的。因此两边都要有覆盖同一组边界的测试。

## 风险

- **计数与列表短暂不一致**：扫描进行中时 `library://status` 会推事件，重取后一致；不推事件的最坏情况是计数停留在上次取值，直到下次失效。可接受——比"完全没有数字"好，且不会显示错误的分页推导值。
- **两条 COUNT 的成本**：`songs` 表上有 `songs_favorites(library_root_uuid, is_favorite, ...)` 索引，`all` 的 `COUNT(*)` 走 root 索引。仅在启动与失效时触发，非热路径。
