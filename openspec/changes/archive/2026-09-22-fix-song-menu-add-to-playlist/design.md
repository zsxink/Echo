## Context

见 `proposal.md`。桌面壳层 `PlaylistsView` 已具备「添加到歌单」所需的全部接线点：

- `addToPlaylistFor` 状态（`PlaylistsView.tsx:89`）
- `AddToPlaylistDialog` 渲染分支（`PlaylistsView.tsx:501-514`）
- 批量菜单的 `onAddToPlaylist` 处理（`PlaylistsView.tsx:312-315`）

缺陷仅在于单曲 `<SongMenu>`（`PlaylistsView.tsx:450-488`）未传入 `onAddToPlaylist`，而 `SongMenu`（`apps/desktop/src/features/library/SongMenu.tsx:190-195`）将「添加到歌单」菜单项在该回调缺失时置为 disabled。

## Goals / Non-Goals

**Goals:**

- 让歌单视图的单曲菜单「添加到歌单」与资料库视图行为一致：可点击、打开 `AddToPlaylistDialog`、确认后写入成员并刷新。
- 复用既有对话组件与批量写入路径，不新增 IPC 命令或状态。
- 通过回归测试覆盖可写歌单可用、只读歌单禁用、确认后刷新。

**Non-Goals:**

- 不改变 `SongMenu` 的通用菜单项语义、`AddToPlaylistDialog` 交互或后端 `add_to_playlists` 行为。
- 不做歌单选择器「排除当前歌单」等新交互——保持与资料库视图一致的现有行为（后端已对已在歌单中的歌曲按 conflict 跳过）。
- 不涉及 Core / IPC / 持久化。

## Decisions

### 1. 在 `PlaylistsView` 的单曲 `SongMenu` 上接入 `onAddToPlaylist`

按 `LibraryWorkspace.tsx:440-443` 的既有模式接线：

```tsx
onAddToPlaylist={() => {
  setAddToPlaylistFor([menuFor.song]);
  setMenuFor(null);
}}
```

`PlaylistsView` 的 `AddToPlaylistDialog` 分支已存在，其 `onDone`（`PlaylistsView.tsx:508-512`）会 `loadMembers()` + `onLibraryChanged?.()` + 退出多选，正好覆盖成员刷新与导航计数刷新。

备选：把该回调做成 `SongMenu` 内部默认行为（无回调时自动打开选择器）。不采用——`SongMenu` 是通用曲库组件，不应隐式依赖 `AddToPlaylistDialog` 或持有歌单视图状态；由拥有查询的视图显式提供回调符合现有架构，也保证只读视图仍能通过「不传回调」实现禁用。

### 2. `readOnly` 语义保持现状

`SongMenu` 的 disabled 表达式 `readOnly || !onAddToPlaylist || unavailable` 已覆盖只读与不可用歌曲两种禁用路径。接线后可写歌单中该菜单项恢复启用，只读歌单与其后置灰保持不变，无需改动 `SongMenu`。

### 3. 不排除「当前歌单」作为目标

与 `LibraryWorkspace`（以及批量入口）行为一致：目标列表包含当前歌单时，后端 `add_to_playlists` 对已存在成员返回 conflict，批量路径将其计为「已在歌单中」跳过。保持这一致性，避免本变更引入与批量入口不同的交互。

## Risks / Trade-offs

- [Risk] 新增一条行为路径（单曲菜单开选择器）可能与既有测试断言冲突。→ 修改前先阅读 `PlaylistsView.test.tsx` 中对菜单项的既有断言，接线后更新受影响测试并新增回归用例。
- [Risk] 歌单选择器里能选到「当前歌单」，产生冗余提示。→ 与批量入口行为一致，属于既有产品行为，不在此变更范围内调整。

## Migration Plan

纯前端接线修复，无需数据迁移或兼容处理。发布后，歌单视图的单曲菜单即刻恢复「添加到歌单」能力；既有用户无需任何迁移动作。

## Open Questions

无。