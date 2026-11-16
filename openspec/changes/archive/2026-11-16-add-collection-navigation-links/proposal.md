# Proposal

## Why

Issue #118 指出：歌手/专辑视图与聚合详情已经可用，但歌曲列表里的歌手和专辑仍是纯文本，用户无法从正在浏览的一首歌直接到达它所属的歌手或专辑。当前 `SongRow` 的行点击统一触发播放，WebView 又拿不到 Core 的规范化分组身份，因此不存在可靠的 deep link 通道。补齐这条链路能让“听到一首歌 → 查看同歌手/同专辑”成为一步操作，同时保持分组身份与目录完全一致。

## What Changes

- **Core 暴露稳定分组身份**：新增公开 helper（如 `collection_keys(&Song)`），用与目录聚合完全相同的规则（生效歌手/专辑 + NFKC + case fold + 空白归一）派生 `artist_key` / `album_key`；仅当原始歌手/专辑元数据非空时返回 key。`SongView` 与 `SongDetailView` 增加可选的 `artistKey` / `albumKey`（camelCase，`skip_serializing_if`），WebView 只原样回传，不按展示名推导。
- **壳层一次性导航目标与来源**：`useAppShell` 增加 `openCollection(kind, artistKey, albumKey?)`、一次性 `collectionTarget` 与跳转来源记录（来源视图 + 歌单身份）；`selectView` 清空未消费目标与来源；不引入前端路由或历史栈。
- **目录应用目标**：`CollectionDirectory` 在目录数据落地后按 key 精确匹配并只消费一次，复用既有详情请求；跳转时清空目录搜索词，避免目标被过滤；目标不存在时停留在可用目录并提示“可能已被移除”。
- **返回来源视图**：从歌曲链接打开的详情，其返回入口回到跳转前的来源视图（全部歌曲/最近添加/喜欢的音乐/歌单/歌手/专辑）；从目录卡片打开的详情仍返回聚合目录。
- **UI 链接**：歌曲列表行（全部歌曲、最近添加、喜欢的音乐、歌单成员、歌手/专辑详情）中的歌手名与专辑名改为可聚焦的 `button`，Enter/Space 可激活，点击不触发行播放、多选切换或批量菜单；多选模式下仍可导航且不改变选择集。缺失元数据的歌曲保持纯文本、不可点击（未知艺人文本保留，缺失专辑维持空白）。
- **覆盖范围（已确认）**：除歌曲列表外，同时覆盖歌曲详情弹窗、常驻播放栏与沉浸式播放器；临时播放项与缺失元数据保持纯文本。常驻播放栏需把“展开播放器”按钮与歌手链接拆成并列可聚焦控件，避免按钮嵌套。
- **规格与追踪**：修改 `artist-album-browsing`、`library-experience`、`desktop-app-shell`、`immersive-lyrics` 四个既有 capability；新增场景按位置派生规则追加到各 Requirement 末尾，并同步 `scenario-commands.mjs`、`docs/traceability.md`、`scripts/verify/manifest.json`。

非目标：

- 不修改分组规则（大小写/全半角归一、跨歌手同名专辑合并保持现状）。
- 不引入前端路由、浏览器式历史栈或可持久化的导航历史。
- 不覆盖歌曲操作菜单头部、播放队列面板、macOS 刘海播放器等未确认界面。
- 不把未知艺人/未知专辑虚拟分组变为可导航目标。
- 不改变播放队列重建、播放统计、收藏或同步语义。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `artist-album-browsing`: 新增“歌曲读取模型暴露稳定分组身份”与“从歌曲链接进入歌手/专辑详情”两项要求；修改“从聚合条目浏览歌曲”，区分从目录条目打开（返回聚合目录）与从歌曲链接打开（返回来源视图）。
- `library-experience`: 新增“歌曲行与歌曲详情中的歌手/专辑链接”要求，规定语义化控件、键盘可达、不触发行播放/多选、未知元数据不可点击。
- `desktop-app-shell`: 新增“一次性聚合导航目标与来源记录”要求，规定 `openCollection` 行为、目标只消费一次、`selectView` 清理残留目标与来源。
- `immersive-lyrics`: 修改常驻播放栏与沉浸式播放器的元信息要求，允许在有稳定身份时把歌手/专辑呈现为可导航链接，且不改变播放状态。

## Impact

- 事实来源：`docs/PRODUCT.md`（本地优先浏览闭环）、`docs/DESIGN.md`（Core 与 UI 分层、Core 拥有规范化规则）、`docs/interface-terminology.md`（歌手目录、专辑目录、聚合详情页、当前播放区、沉浸式播放器）、`openspec/CODE_STANDARDS.md` §2 文件规模、§3 分层与依赖方向、§6 类型化 IPC、§7 前端测试与交付门槛。
- Core：`crates/echo-core/src/application/catalog.rs`（新增公开 key 派生 helper，目录聚合复用）、`crates/echo-core/src/application/detail.rs`（`SongDetail` 增加可选 key）及对应测试。
- 桌面 Rust：`crates/echo-desktop/src/ipc/dto.rs`（`SongView`、`SongDetailView` 增加可选 key，映射 Core helper）及 DTO 序列化测试；无数据库迁移、无持久化字段。
- 生成物：`apps/desktop/src/ipc/ipc-types.generated.ts` 由 `pnpm --dir apps/desktop generate:ipc` 重新生成，禁止手改。
- 前端壳层：`apps/desktop/src/app/useAppShell.ts`、`apps/desktop/src/app/App.tsx`；新增轻量 `CollectionNavigationProvider`（无 provider 时链接退化为纯文本，保护既有单测与独立渲染）。
- 前端界面：`features/library/SongRow.tsx`、`SongList.tsx`、`CollectionDirectory.tsx`、`SongDetail.tsx`、`SongMenu.tsx`、`features/player/PlayerBar.tsx`、`features/player/ImmersivePlayer.tsx`、`features/player/useSongDetail.ts` 及样式（`styles/library.css`、`styles/player.css`、`styles/responsive.css`）。
- 兼容性：DTO 仅新增可选字段，旧消费方无需改动；无 IPC command 签名变化；无 BREAKING。
- 治理与追踪：`artist-album-browsing` 不在 `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 注册表中；`library-experience`、`desktop-app-shell`、`immersive-lyrics` 已注册，其新增 Scenario 必须同步 `scripts/verify/scenario-commands.mjs`、`docs/traceability.md`，再用 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json` 与 `tests/scenarios|native`，最后跑 `node scripts/verify/reconcile-scenarios.mjs`；`docs/prd-matrix.md` 按既有手工流程同步。
- 关联 Issue：#118（本 change 的实现与 PR 必须关联）。
