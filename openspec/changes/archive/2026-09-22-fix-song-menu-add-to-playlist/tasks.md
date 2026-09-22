## 1. Wiring

- [x] 1.1 在 `PlaylistsView.tsx` 的单曲 `<SongMenu>` 上接入 `onAddToPlaylist`（复用 `addToPlaylistFor` 状态，模式同 `LibraryWorkspace`），并验证 `pnpm --filter @echo/desktop typecheck` 通过

## 2. Regression Coverage

- [x] 2.1 阅读 `PlaylistsView.test.tsx` 中歌单视图单曲菜单的既有断言，接线后更新冲突断言并新增「可写歌单中『添加到歌单』可点击且打开选择器」的回归用例，验证该测试通过
- [x] 2.2 新增/确认「只读歌单中『添加到歌单』保持禁用」的回归用例，验证该测试通过

## 3. Verification

- [x] 3.1 运行 `pnpm --filter @echo/desktop lint`，确认无 lint 错误
- [x] 3.2 运行 `pnpm --filter @echo/desktop test -- --run`，确认歌单相关测试全部通过
- [x] 3.3 运行 OpenSpec 校验（`openspec validate --change fix-song-menu-add-to-playlist`），确认 change 产物通过
- [x] 3.4 运行已归档既有验证中相关的手工/原生验收场景（`scripts/verify/checks/task-10.9.mjs`），确认歌单视图单曲菜单可用