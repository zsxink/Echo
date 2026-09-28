## 1. 浮层栈：触发控件参与内部区域判定

- [x] 1.1 在 `apps/desktop/src/app/overlays.ts` 的 `OverlayOptions` 增加可选 `triggerRef`，`Registration` 同步持有，并把外部 `pointerdown` 的内部区域判定改为容器 `contains` 与 `triggerRef` `contains` 取并（design D1）；导出一个供组件复用的内部判定函数，Escape 处理器与 `useFocusTrap` 不读取该字段（design D2）。
- [x] 1.2 在 `apps/desktop/src/app/overlays.test.tsx` 补充回归：传入 `triggerRef` 的浮层在再次点击触发控件时关闭且不被后续 `click` 重新打开；不传 `triggerRef` 的浮层行为不变；点击真正外部区域仍按既有规则关闭。运行 `pnpm --filter @echo/desktop exec vitest run src/app/overlays.test.tsx`。

## 2. 播放队列面板的触发按钮接线

- [x] 2.1 在 `apps/desktop/src/app/App.tsx` 创建一个 `queueTriggerRef` 并同时传给 `QueuePanel` 与 `PlayerBar`；`QueuePanel` 的 `useOverlay` 传入该 ref（design D6 接线部分），`PlayerBar` 把它挂到 `data-testid="queue-trigger"` 的按钮上，不改 `.queue-popover` 的定位与任何 CSS（design Context 关键约束）。
- [x] 2.2 把 `PlayerBar.tsx` 中播放队列按钮的 `aria-label` 改为随 `ui.queueOpen` 在「显示播放队列」/「隐藏播放队列」间切换，`aria-expanded` 保持不变（design D6）。
- [x] 2.3 在 `apps/desktop/src/features/player/QueuePanel.test.tsx` 补充回归：面板打开后再次点击 `queue-trigger` 则面板收起、`aria-expanded` 回到关闭态；可访问名称随状态切换。运行 `pnpm --filter @echo/desktop exec vitest run src/features/player/QueuePanel.test.tsx src/features/player/PlayerBar.test.tsx`。

## 3. 歌曲操作菜单与批量操作菜单的触发控件接线

- [x] 3.1 在 `SongRow.tsx` 把行元素（`event.currentTarget`）作为触发控件节点回传，在 `SongList.tsx` 与 `LibraryWorkspace.tsx`、`PlaylistsView.tsx`、`CollectionDirectory.tsx` 之间穿传对应 ref（design D4、D5）；`SongList`/`SongRow` 的新 prop 提供默认值，不影响既有调用方与测试。
- [x] 3.2 `SongMenu.tsx` 的 `useOverlay` 传入触发 ref，并把它自己的 `document` 级 `pointerdown` 监听改为复用 `overlays.ts` 导出的同一判定函数，保留 `dismissable` 开关语义（design D3）；`BatchSongActions.tsx` 的 `BatchSongMenu` 传入触发 ref。
- [x] 3.3 在 `SongMenu.test.tsx` 与 `BatchSongActions.test.tsx` 补充回归：由行内入口打开的菜单再次点击该入口即收起，且不触发该行的播放或其它行内动作；点击菜单外部仍关闭；在删除确认框或详情浮层打开时关闭判定仍按既有规则暂停。运行 `pnpm --filter @echo/desktop exec vitest run src/features/library/SongMenu.test.tsx src/features/library/BatchSongActions.test.tsx src/features/library/SongRow.test.tsx src/features/library/SongList.test.tsx`。

## 4. 桌面 UI 交付验证

- [x] 4.1 对改动文件运行 Prettier 检查、桌面 ESLint 与 TypeScript 检查：`pnpm --filter @echo/desktop exec prettier --check src/app/overlays.ts src/app/overlays.test.tsx src/app/App.tsx src/features/player/QueuePanel.tsx src/features/player/QueuePanel.test.tsx src/features/player/PlayerBar.tsx src/features/library/SongMenu.tsx src/features/library/SongMenu.test.tsx src/features/library/BatchSongActions.tsx src/features/library/BatchSongActions.test.tsx src/features/library/SongList.tsx src/features/library/SongRow.tsx src/features/library/LibraryWorkspace.tsx src/features/playlists/PlaylistsView.tsx`。
- [x] 4.2 运行桌面 UI 全量 Vitest 与生产构建：`pnpm --filter @echo/desktop test`、`pnpm --filter @echo/desktop build`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`；确认 `openspec validate fix-queue-trigger-toggle --strict` 通过。
