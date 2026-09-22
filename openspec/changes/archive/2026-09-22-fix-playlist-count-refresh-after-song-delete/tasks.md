## 1. Refresh Coordination

- [x] 1.1 在普通曲库视图中将歌曲删除/撤销成功后的回调组合为“刷新歌曲列表并刷新歌单导航”，并验证 `LibraryWorkspace` 的相关 TypeScript 测试通过
- [x] 1.2 在歌单视图中将歌曲删除/撤销成功后的回调组合为“刷新歌单成员并刷新歌单导航”，并验证 `PlaylistsView` 的相关 TypeScript 测试通过

## 2. Regression Coverage

- [x] 2.1 增加从普通曲库删除已入歌单歌曲后重读 `playlists` 的回归测试，并验证删除失败不触发该重读
- [x] 2.2 增加从歌单视图删除/撤销歌曲后同步更新导航计数的回归测试，并验证 `pnpm --filter @echo/desktop test -- --run` 通过

## 3. Verification

- [x] 3.1 运行 `pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop lint` 与 OpenSpec 校验，确认类型、规范和变更产物均通过
