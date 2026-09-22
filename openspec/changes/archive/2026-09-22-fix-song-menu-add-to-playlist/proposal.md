## Why

在歌单视图（`PlaylistsView`）中，点击歌曲行「三个点」打开的单曲操作菜单里，「添加到歌单」菜单项是禁用（置灰）状态——`PlaylistsView` 渲染 `<SongMenu>` 时遗漏了 `onAddToPlaylist` 回调，而该菜单项在回调缺失时恒为 disabled。同一菜单在「全部歌曲」等资料库视图中正常可用。这违反了既有规格「歌单成员添加」要求（支持从歌曲操作菜单将歌曲添加到歌单），也使歌单内歌曲失去从单曲菜单加入其他歌单的能力；仅批量菜单入口可用。

## What Changes

- 在 `PlaylistsView` 渲染的单曲 `<SongMenu>` 上接入 `onAddToPlaylist`，复用已有的 `addToPlaylistFor` 状态与 `AddToPlaylistDialog` 渲染分支（与 `LibraryWorkspace` 的接线一致）。
- 使歌单视图的单曲「添加到歌单」可点击，打开歌单选择器；确认后新增成员并刷新成员列表与歌单导航计数。
- 保持 `readOnly`（不可写）歌单下该菜单项继续禁用，不改变不可用歌曲的禁用语义。
- 增加歌单视图的回归测试，覆盖单曲菜单「添加到歌单」可点击并打开选择器；同步更新既有相关测试若其断言了旧的不可用行为。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `playlist-management`: 「歌单成员添加」要求从歌曲操作菜单或等效单曲入口可将歌曲（含歌单视图内的歌曲）添加到一个或多个歌单；本变更修复歌单视图中该入口不可用的问题，使菜单入口在可写歌单视图中保持一致可用。

## Impact

- 受影响模块：`apps/desktop/src/features/playlists/PlaylistsView.tsx`（`<SongMenu>` 接线）、`apps/desktop/src/features/playlists/AddToPlaylistDialog.tsx`（已具备，无需改动）、相关 React/Vitest 回归测试（`apps/desktop/src/features/playlists/PlaylistsView.test.tsx`）。
- 不修改 IPC 协议、Core 领域逻辑、数据模型或持久化结构；后端 `playlists` / `add_to_playlists` 命令继续提供权威歌单数据与成员写入。
- 仅涉及桌面 presentation 层接线与测试，遵守桌面 UI 与 Rust Core 分层，Core 不依赖 UI。