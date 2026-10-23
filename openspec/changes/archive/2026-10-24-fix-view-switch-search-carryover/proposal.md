# Proposal

## Why

带搜索框的列表在视图切换后保留旧搜索词，并继续过滤新视图（issue #47）：歌单 A 搜「晴天」再切到歌单 B，B 显示的是「在 B 里搜晴天」的结果，用户会误以为 B 里就这些歌。三处同形状列表（歌单、全部歌曲/最近添加/喜欢的音乐、歌手/专辑目录）在 `App` 中由同一组件实例渲染，切换只换 props，而搜索词是组件本地 `useState`，于是跟着实例存活；`歌单 A → 全部歌曲 → 歌单 B` 这类跨组件路径不复现，所以一直未被发现。

同文件里排序已按 scope 派生（`useStoredSongSort`：切换视图不借用上一视图的排序），搜索词缺少对应的处理。规格侧 `library-experience` 的「切换全部歌曲」场景写有「保留当前搜索与排序控件的明确状态」，被实现解读为保留搜索词，与产品意图（搜索属于被离开的视图）冲突，需一并修正。

## What Changes

- 三处组件把搜索清空并入既有的「导航即重置」副作用：
  - `PlaylistsView`：`playlistId` 变化时清空 `searchText`；派生的 `searched`/`searching`/`searchCursor` 由既有空词分支复位（与手动清空同一条路径）。
  - `LibraryWorkspace`：`view` 变化的重置副作用里加 `setSearch("")`。
  - `CollectionDirectory`：`kind` 变化的重置副作用里加 `setSearch("")`。
- 切换时已发出的旧词请求被既有代际计数丢弃（`searchRequest` / `useSongs` 的 reqId / `directoryRequest`），迟到结果不回写。
- 不采用给组件加 `key` 强制重挂载：会顺带清掉选中态、批量模式、菜单与滚动位置，而这些已各有精确 effect。
- 修正 `library-experience`「资料库视图」的「切换全部歌曲」场景文本（搜索词不再保留），并新增「切换视图清空搜索」「切换歌手或专辑目录清空搜索」两个场景；`playlist-management`「歌单详情搜索」新增「歌单切换清空搜索」场景。
- 三个视图各补一条「切换视图清空搜索」组件回归测试，并同步场景三方对账（traceability / scenario-commands / manifest）。

非目标：不改后端搜索与分页契约；不改歌单 A → 全部歌曲这类跨组件路径（本就卸载重置）；不改排序的按 scope 持久化；不处理聚合详情（打开某歌手/专辑）内的搜索保留行为。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `library-experience`: 「资料库视图」要求明确「切换视图清空搜索」——歌曲视图互切与歌手/专辑目录互切均不得沿用旧搜索词，并修正「切换全部歌曲」场景中与实现冲突的「保留当前搜索」表述。
- `playlist-management`: 「歌单详情搜索」要求补齐「歌单切换清空搜索」——切换歌单时搜索词与已加载搜索结果一并清空，新歌单呈现完整成员。

## Impact

- **桌面前端（Presentation）**：`apps/desktop/src/features/playlists/PlaylistsView.tsx`、`features/library/LibraryWorkspace.tsx`、`features/library/CollectionDirectory.tsx` 各加一行重置；对应三个 `.test.tsx` 各加一条用例。
- **规格（spec）**：`library-experience` 与 `playlist-management` 的 delta spec；归档时同步回主规格（LE-R01 追加 S05/S06、PM-R09 追加 S05，均为末尾追加，不重排既有 ID）。
- **可追溯性资产**：`docs/traceability.md`、`scripts/verify/scenario-commands.mjs` 登记三个新场景，`gen-scenario-manifests.mjs` 重生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*.yaml`。
- 不触及 Rust Core、IPC 契约与 e2e。
