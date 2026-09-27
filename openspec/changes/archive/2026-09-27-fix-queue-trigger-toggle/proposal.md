## Why

播放栏的「播放列表」按钮打开播放队列面板后，再次点击不会收起：面板先被关闭、又在同一次点击的 `click` 阶段重新打开，净效果为零。根因是浮层栈的「点击外部关闭」只把浮层自身的容器当作内部区域，而触发按钮不在该容器内。同一机制也影响歌曲操作菜单和批量操作菜单——它们同样是 `Menu` 层且触发按钮在浮层容器之外。

## What Changes

- 为 `useOverlay` 增加可选的触发控件 ref：外部 `pointerdown` 判定内部区域时，把它与容器 `contains` 结果取并，使触发按钮再次点击能够正常切换浮层。
- 播放队列面板的播放栏触发按钮参与该判定，面板开合后按钮的 `aria-expanded` 同步变化。
- 歌曲操作菜单与批量操作菜单的触发控件参与同一判定：在三条入口（曲库工作区、歌单详情、歌手/专辑聚合页）上均可由触发控件再次点击关闭。
- 播放队列触发按钮的可访问名称随开合状态切换，面板打开时不再读作「显示播放队列」。
- 保持浮层栈的既有契约不变：Escape 优先级、焦点进入与恢复、Tab 焦点陷阱、点击真正外部区域仍只关闭最上层菜单。
- 不改变播放队列的条目投影、排序与清空规则，不改变右键菜单的打开方式、菜单项集合或批量操作语义。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `desktop-app-shell`：明确浮层的触发控件属于其「内部区域」，可由触发控件再次点击收起，与浮层关闭后焦点回到触发控件的既有规则一致。
- `library-experience`：明确歌曲操作菜单与批量操作菜单的触发控件可再次点击关闭，且不影响点击真正外部区域的既有行为。

## Impact

- 影响 `apps/desktop/src/app/overlays.ts`（`OverlayOptions` 与外部交互判定）、`apps/desktop/src/app/App.tsx`、`apps/desktop/src/features/player/QueuePanel.tsx`、`PlayerBar.tsx`、`apps/desktop/src/features/library/SongMenu.tsx`、`BatchSongActions.tsx`、`SongList.tsx`、`SongRow.tsx`，以及 `LibraryWorkspace.tsx`、`PlaylistsView.tsx`、`CollectionDirectory.tsx` 中把它们接线的 props。
- 影响上述模块的 React 测试：补充触发控件二次点击的回归覆盖。
- 不改变 IPC、Tauri command、Core、数据库或持久化格式，不新增依赖。
- 仅涉及 React 展示层，不触碰 `openspec/CODE_STANDARDS.md` §3.1 的分层边界；符合 `desktop-app-shell` 既有的浮层关闭与可访问性约定，术语沿用 `docs/interface-terminology.md`。
- 关联用户反馈：[Issue #33](https://github.com/zsxink/Echo/issues/33)。
