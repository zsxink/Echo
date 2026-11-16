# Design

## Context

- `SongRow` 的歌手/专辑目前是纯文本；整行点击在 `fromRowControl` 处只放行 `button`，所以把文本换成按钮即可天然避免触发行播放，但链接需要拿到 Core 的分组身份。
- 目录聚合身份由 `normalized_key`（NFKC + Unicode case fold + 空白归一）决定；专辑只按规范化专辑名分组，不按歌手拆分。WebView 无法等价复刻该规则，因此 key 必须由 Core 派生。
- `CollectionDirectory` 的“当前选中条目”是组件内部 state；shell 的 `selectView` 只能切视图，不能携带目标。需要新增一条一次性的 shell 导航通道。
- `SongView` 由桌面 IPC 层映射，`SongDetailView` 由 Core `SongDetail` 映射；两者都是只读模型，无持久化字段。
- 常驻播放栏的当前播放区当前是单个 `<button class="player-track-card">`，封面、标题、歌手文本都嵌在其中；直接加入歌手链接会产生非法的按钮嵌套，需要拆分控件。
- 新增场景会进入 `docs/traceability.md` 与场景清单；场景 ID 由 spec 中的位置派生，因此新增 Requirement 只能追加在 spec 末尾，新增 Scenario 只能追加在所属 Requirement 末尾。

## Goals / Non-Goals

**Goals:**

- 让歌曲列表、歌曲详情、常驻播放栏、沉浸式播放器都能用同一份 Core 身份导航到歌手/专辑详情。
- 目标应用只消费一次、可降级、可返回来源视图，且不引入路由或历史栈。
- 保持既有单测/独立渲染可用：没有导航 provider 时，所有链接退化为纯文本。
- DTO 变化保持增量兼容，不触碰数据库、同步载荷或播放队列语义。

**Non-Goals:**

- 不恢复来源视图的瞬时搜索词（既有视图切换同样不保留）。
- 不为歌曲操作菜单头部、播放队列面板、macOS 刘海播放器增加链接。
- 不改变目录聚合规则或“未知艺人/未知专辑”虚拟分组的存在性。
- 不引入前端路由库、URL deep link 或跨会话导航历史。

## Decisions

### D1. 分组身份由 Core 的领域 helper 统一派生

在 `crates/echo-core/src/domain/catalog.rs` 提供：

- `pub fn effective_artist(&Song) -> &str` / `pub fn effective_album(&Song) -> &str`：把缺失或空白元数据映射为“未知艺人”/“未知专辑”，供目录聚合与排序复用。
- `pub struct CollectionKeys { pub artist_key: Option<String>, pub album_key: Option<String> }` 与 `pub fn collection_keys(&Song) -> CollectionKeys`：仅当原始元数据 trim 后非空时返回 `Some(normalized_key(...))`，否则 `None`。
- `application/catalog.rs` 删除私有 `effective_artist` / `effective_album`，改用领域 helper；目录聚合与详情筛选的 key 规则因此与 `collection_keys` 共用同一实现。

选择 `Option<String>` 而不是始终返回“未知艺人”key：已确认“未知元数据不可点击”，key 的存在性直接表达“可导航”，避免 UI 再依据展示名判断。选择放在 domain 而非 application：分组身份是领域概念，application 与 desktop 都只是消费方，符合依赖方向。

### D2. DTO 增量暴露 key，不新增持久化字段

- Core `SongDetail` 增加 `artist_key: Option<String>` / `album_key: Option<String>`。
- 桌面 `SongView` 与 `SongDetailView` 增加同名可选字段，serde camelCase + `skip_serializing_if = "Option::is_none"`，映射时调用 `collection_keys`。
- `ipc-types.generated.ts` 通过 `pnpm --dir apps/desktop generate:ipc` 重新生成，禁止手改。

选择在 `SongDetailView` 也暴露 key（而不只 `SongView`）：常驻播放栏与沉浸式播放器使用 `useSongDetail` 读取权威元信息，没有 row 的 `SongView`。这是纯增量字段，旧消费方与旧数据均无需迁移；无 SQLite 迁移、无同步载荷变化。

### D3. shell 持有一次性目标与单次来源，而不是路由

`useAppShell` 增加：

```ts
type CollectionTarget =
  | { kind: "artist"; artistKey: string }
  | { kind: "album"; albumKey: string };

interface CollectionSource {
  view: LibraryViewKind;
  playlistId: string | null;
}
```

- `openCollection(target)`：记录来源（仅在当前没有待消费目标时写入，快速连续点击保留第一次来源）、设置 `libraryView` 为 `artists`/`albums`、清空 `activePlaylistId`、关闭窄屏侧边栏、设置 `collectionTarget`。它不能复用 `selectView`，因为后者要清空目标。
- `consumeCollectionTarget()`：只清目标，保留来源，供详情返回。
- `returnToSource()`：用来源调用 `selectView(source.view, source.playlistId)`。
- `selectView`：无论从侧边栏、应用菜单、歌单删除回退还是其他地方调用，都清空目标与来源，满足“离开即失效”。

目标用判别联合而非 `artistKey + albumKey?`：专辑身份只由 albumKey 决定，携带歌曲自己的 artistKey 会在跨歌手同名专辑时引入错误身份；专辑详情的 `catalog_collection_songs` 需要 artistKey 参数，但该参数在 album kind 下被 Core 忽略，且目录匹配后可直接使用条目自己的代表歌手 key。

不引入历史栈：来源只有一条，返回一次即清空；重复跳转会覆盖目标、保留第一次来源。

### D4. CollectionDirectory 的“目标应用”协议

`CollectionDirectory` 新增 `target`、`onTargetConsumed`、`onReturnToSource`、`returnLabel` props，并维护内部 `openedFromTarget`：

1. `target` 变化且 kind 匹配时：清空目录搜索词；若搜索词原本非空，借搜索 effect 重新请求；同时递增 `refreshEpoch` 强制一次目录响应，覆盖“目录已加载后才收到目标”的情况。
2. 在 `catalog_collections` 的成功回调中（请求代次校验之后）读取最新 target ref：`artist` 按 `artistKey`、`album` 按 `albumKey` 精确匹配；命中则 `setSelected(entry)`、`openedFromTarget = true`、调用 `onTargetConsumed()`。
3. 未命中：保持目录视图，`notify("未找到该歌手/专辑，可能已被移除")`，并消费目标，避免后续刷新反复提示。
4. 目录请求失败：保留目标等待用户重试，不消费；既有错误横幅继续可用。
5. 用户从卡片手动打开条目时 `openedFromTarget = false`；`kind`/`root` 变化时重置。返回按钮在 `openedFromTarget && onReturnToSource` 时调用来源返回，否则沿用 `setSelected(null)` 回目录；`aria-label` 使用 `returnLabel`（如“返回全部歌曲”“返回歌单”）。

在响应回调里应用而不是在渲染期匹配：目录响应是唯一权威的条目集合，能同时覆盖首屏、刷新和迟到响应；`refreshEpoch` 只用于触发一次新响应，不改变既有请求代次与根目录隔离逻辑。

### D5. 导航能力经可选 context 下发

新增 `features/library/collectionNavigation.tsx`：

- `CollectionNavigationProvider` 接收 `{ openArtist, openAlbum }`。
- `useOptionalCollectionNavigation()` 在没有 provider 时返回 `null`；`SongRow`、`SongDetail`、`PlayerBar`、`ImmersivePlayer` 据此决定渲染链接还是纯文本。

`App` 在顶层（`ShellNavProvider` 内、覆盖工作区与播放器）安装 provider，值来自 `useAppShell().openCollection`。选择 context 而非逐层 props：`SongRow` 同时被 `LibraryWorkspace`、`CollectionDirectory`、`PlaylistsView` 三条链路渲染，props 方案要改三处调用方和全部中间组件；context 还让播放器表面无需从 shell 逐层传参。无 provider 时退化为纯文本，保护既有组件测试。

### D6. 各表面的链接形态

- **SongRow**：`.track-artist` / `.track-album` 内有 key 时渲染 `<button type="button" class="track-artist-link|track-album-link">`，`aria-label="打开歌手 X"` / `"打开专辑 X"`；点击 `stopPropagation`，不改变选择集。可用性标签留在按钮外；缺失 key 时保留原纯文本（未知艺人 / 空白专辑）。
- **SongDetail**：`<dd>` 内同样渲染链接；激活时先调用 `onNavigate()` 关闭详情与歌曲操作菜单，再调用 context 导航。`SongMenu` 只新增一个把 `onClose` 传下去的 prop，不改变菜单项。
- **PlayerBar**：把 `.player-track-card` 从单个 button 改为容器；封面+标题保留一个“展开播放器”按钮，歌手链接作为并列 button 放在文本列中。这样避免 button 嵌套，同时保留 `aria-expanded` 与原有展开入口；无 key 时仍渲染原 subline 文本（艺人或时长）。
- **ImmersivePlayer**：`.song-meta` 中的艺人/专辑改为内联 button；激活时先 `setImmersiveOpen(false)`（并关闭歌词专注），再导航，确保浮层不会覆盖新视图。
- 样式统一给链接按钮做按钮重置（无背景/边框/内边距，继承字号与颜色，`cursor: pointer`），补 `:hover` 与 `:focus-visible`，保留 ellipsis。

### D7. 未知元数据与临时播放项

`collection_keys` 对空白元数据返回 `None`；临时播放项没有 `SongDetailView`，同样没有 key。两类情况统一渲染纯文本，不产生可聚焦控件，不进入虚拟分组。目录仍按既有规则展示“未知艺人/未知专辑”条目，只是歌曲行不提供进入它们的链接。

### D8. 场景与追踪同步

新增 Requirement 追加在各 spec 末尾，新增 Scenario 追加在所属 Requirement 末尾，避免位置派生 ID 顶掉既有场景。`artist-album-browsing` 不在 `AREA_PREFIX` 注册表中，无需进入场景清单；`library-experience`、`desktop-app-shell`、`immersive-lyrics` 的新场景预计为 `LE-R23-S01..S06`、`DAS-R29-S01..S04`、`IL-R01-S04`、`IL-R03-S04`（以同步后的 `allScenarioIds()` 为准）。实现完成后：

1. 在 `scripts/verify/scenario-commands.mjs` 为新场景补单条可执行命令。
2. 同步 `docs/traceability.md` 行（任务层/测试层沿用相邻行）。
3. `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `manifest.json` 与 `tests/scenarios|native`。
4. `node scripts/verify/reconcile-scenarios.mjs` 校验 spec = trace = manifest。
5. 按既有手工流程同步 `docs/prd-matrix.md`。

## Risks / Trade-offs

- **目标应用需要额外一次目录请求** → 只在跳转时递增 `refreshEpoch`，不改变常规浏览路径；请求失败保留目标供重试。
- **返回来源不恢复搜索词** → 与既有视图切换语义一致，规格已明确“不要求恢复”；避免为瞬时搜索引入全局状态。
- **播放栏结构变化可能影响布局与既有测试** → 保留原 class 与 `aria-expanded`，只拆分交互控件；为展开入口与歌手链接分别补测试，并跑视觉相关断言。
- **快速连续点击多个链接** → 目标后写覆盖前写，来源保留第一次；目标只消费一次，不会排队打开多个详情。
- **跨歌手同名专辑** → album key 只含专辑名，链接落到合并条目，符合既有聚合规则；UI 不承诺歌手区分。
- **目标未命中提示可能被忽略** → 使用既有非阻塞 toast；目录保持可用，用户仍可手动浏览。
- **DTO 增量字段** → 旧前端/旧载荷不受影响；生成物 drift 由治理门校验。

## Migration Plan

无数据迁移。发布顺序：Core helper 与 DTO → 重新生成 IPC 类型 → 前端 provider/shell → 各表面链接 → 场景与追踪同步。回滚只需还原代码并重新生成 IPC 类型；已存在的 key 字段是可选只读字段，不影响旧数据。

## Open Questions

无。三项范围决策（返回来源视图、未知元数据不可点击、覆盖详情弹窗与播放器）已在规划前确认。
