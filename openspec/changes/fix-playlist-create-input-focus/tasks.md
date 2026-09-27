## 1. 嵌套焦点回归与修复

- [x] 1.1 在 `AddToPlaylistDialog.test.tsx` 覆盖从选择器打开命名弹窗、点击并输入（含 composition 事件）、取消后焦点返回及选择器状态保留；运行 `pnpm --filter @echo/desktop exec vitest run src/features/playlists/AddToPlaylistDialog.test.tsx` 并确认回归断言可验证行为。
- [x] 1.2 让 `useOverlay` 通过最新回调 ref 更新关闭行为而不重置 overlay 生命周期，移除命名输入框重复的 `autoFocus`，并保留父选择器的 Escape 注册；运行选择器集成测试及 `pnpm --filter @echo/desktop exec vitest run src/app/overlays.test.tsx`。

## 2. 桌面 UI 交付验证

- [x] 2.1 对改动文件运行 Prettier 检查、桌面 ESLint 和 TypeScript 检查：`pnpm --filter @echo/desktop exec prettier --check src/features/playlists/PlaylistNameDialog.tsx src/features/playlists/AddToPlaylistDialog.test.tsx src/app/overlays.ts src/app/overlays.test.tsx`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`。
- [x] 2.2 运行桌面 UI 全量 Vitest 和生产构建：`pnpm --filter @echo/desktop test`、`pnpm --filter @echo/desktop build`；确认 `openspec validate fix-playlist-create-input-focus --strict` 通过。
