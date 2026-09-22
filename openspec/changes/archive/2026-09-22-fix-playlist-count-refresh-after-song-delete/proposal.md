## Why

删除一首已经加入歌单的歌曲后，后端会移除它在所有歌单中的成员关系，且“全部歌曲”导航数量会立即刷新，但歌单侧边栏仍显示删除前的成员总数。这个陈旧计数会让用户误以为歌曲仍属于歌单，也与歌单实际内容不一致。

## What Changes

- 在歌曲删除成功后的统一刷新路径中，同时刷新当前歌曲视图和歌单导航列表。
- 在删除撤销成功后，同样刷新歌单导航列表，使成员数量恢复为后端真实值。
- 覆盖从“全部歌曲”或歌单视图发起的单曲删除，并保持删除失败时歌单计数不变。
- 增加前端回归测试，验证删除/撤销会触发歌单列表重读，而失败不会伪造刷新。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `playlist-management`: Echo 删除歌曲并提交后，所有受影响歌单的导航成员数量必须同步反映成员关系的移除；撤销成功后恢复该数量。

## Impact

- 受影响模块：`apps/desktop/src/features/library/LibraryWorkspace.tsx`、`apps/desktop/src/features/playlists/PlaylistsView.tsx`、歌曲菜单删除回调及其 React/Vitest 回归测试。
- 不修改 IPC 协议、Core 数据模型或持久化结构；继续由后端 `playlists` 查询提供权威成员数量。
- 遵守桌面 UI 与 Rust Core 分层：刷新协调留在桌面 presentation 层，Core 仍不依赖 UI。
