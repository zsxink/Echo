## 1. 歌单内搜索分页接线

- [x] 1.1 在 `PlaylistsView.tsx` 增加 `nextCursor` 的 `useRef`（游标存 ref 不触发渲染），把 `playlistsMemberSearch` 从「一次性查询」改为接受可选 `cursor` 参数的续页调用：传 `cursor` 时用 `cursor: nextCursorRef.current` 取代硬编码的 `cursor: null`，返回时把 `nextCursor` 写入 ref（D3）；搜索词/排序/歌单变化时 ref 归位为空，保持既有 `++searchRequest.current` 序号裁决不变（D2）
- [x] 1.2 实现 `loadMoreSearchResults` 回调：入口检查「搜索态 + 游标非空 + 未在飞行中」，满足则以当前游标续页，把 `items` 追加到既有 `searched.items`、`totalCount` 沿用后端返回值、`isLast`/`nextCursor` 更新为最新一页的值；**不得**因续页重置 `totalCount`（否则标题计数会随加载漂移，违反 spec 的「完整命中总数」）
- [x] 1.3 把 `SongList` 的 `onLoadMore={() => {}}` 替换为 `onLoadMore={loadMoreSearchResults}`。非搜索态保持空操作（成员列表 `isLast` 硬编码为 `true`，`SongList` 不会触发），成员列表取数路径不变；确认 `SongList.tsx:184-191` 的滚动哨兵与 `SongList.tsx:133-154` 的定位 effect 两条触发路径都接入
- [x] 1.4 运行 `pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx` 与 `pnpm --filter @echo/desktop typecheck`，确认既有 15 个用例（含搜索三态、定位、批量、异步失败路径）全部通过、无类型回归

## 2. 分页与竞态的组件测试

- [x] 2.1 在 `PlaylistsView.test.tsx` 的 `PlaylistsView — 歌单内搜索 (PLA-SRCH)` 块新增「跨页加载」用例：mock 首屏返回 `isLast: false` + `nextCursor`，滚动 `song-list` 触发 `onLoadMore`，断言第二次 `search` 携带该游标、且返回行**追加**到首屏之后（而非替换），标题计数仍为完整命中数
- [x] 2.2 新增「末页终止」用例：mock 最后一页 `isLast: true`、滚动到底部，断言不再发起 `search`（用 `bridge.call` 的调用次数或参数断言）
- [x] 2.3 新增「请求竞态失效」用例：让首屏 `search` 返回延迟 promise，在其飞行中改变搜索词，断言迟到的首屏结果未渲染进列表（对齐 `useSongs.test.tsx` 的 `discards a delayed old-root response after the active root changes` 写法：先 resolve 新请求，再 resolve 旧请求并断言旧行不在 DOM）
- [x] 2.4 运行 `pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx` 确认三个新用例与全部既有用例通过

## 3. 可追溯性与规格同步

- [x] 3.1 手工向 `docs/traceability.md` 追加新场景行（`LE-R18-S05`–`S08` 歌单搜索结果跨页加载/到达末页后停止续页/搜索条件变化重置分页/迟到的旧条件结果不得显示，以及 `LE-R18-S09` 歌单搜索标题显示完整命中数；`PM-R09-S03`–`S04` 歌单详情搜索结果可续页/末页不再续页），任务列写本 change 的任务号，测试层写 `React`，manifest 列指向 `tests/scenarios/<ID>.yaml`
- [x] 3.2 手工向 `scripts/verify/scenario-commands.mjs` 追加每个新 ID 的验收命令（用 `-t "<test name>"` 过滤到 2.1–2.3 新增的用例名）
- [x] 3.3 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*.yaml`（生成物，禁止手工编辑或 `json.dump(indent=2)`），再运行 `node scripts/verify/gen-scenario-manifests.mjs` 确认不再报漂移
- [x] 3.4 运行 `node scripts/verify/reconcile-scenarios.mjs` 确认 spec / traceability / manifest 三方 ID 集合相等；运行 `node scripts/verify/prd-matrix.mjs` 重生成 PRD 矩阵
- [x] 3.5 运行 `pnpm verify:scenario -- <每个新 ID>` 逐项执行并全部通过

## 4. 最终复核

- [x] 4.1 运行 `pnpm --filter @echo/desktop lint` 与 `pnpm --filter @echo/desktop typecheck` 全绿；运行 `pnpm --filter @echo/desktop test` 全量，确认资料库视图 `useSongs`/`SongList` 既有分页与定位测试无回归
- [x] 4.2 运行 `cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`（本次不改 Rust，应为无变化基线）；运行 `openspec validate --strict fix-playlist-search-pagination` 通过
- [x] 4.3 手动验证（**以运行时观察替代应用内手动走查，理由见下**）
- [x] 4.4 CR 复核：核对后端游标契约（末页 `next_cursor` 确为 `None`，不会无限续页；`total_count` 在游标谓词之前计算，故与分页无关；`limit` 上界 500，前端 200 合法），并补入 CR 发现的两处（D8）

### 4.3 的执行方式变更（记录，非静默降级）

原计划用 `apps/desktop/e2e/` 在真实 Tauri 运行时里走一遍。该 harness 无法执行本条：

- `mock-bridge.ts` 的 `paged()` 助手只会造单页（`nextCursor: undefined`、`isLast: true`、切 100 条），永远发不出第二页；
- 它不按歌单范围裁剪搜索结果，无法复现「歌单内命中 > 200」；
- `run-e2e.mjs` 只对歌词面板改 `scrollTop`，从不滚动歌曲列表，滚动哨兵不会被触发。

扩写 e2e harness 属于本 change 范围之外。因此改用**针对真实组件的可执行运行时观察**替代一次性人工走查：用真实 `PlaylistsView` + `SongList`，把 `clientHeight`/`scrollHeight` 设为真实几何（`ROW_HEIGHT = 44`）使虚拟窗口与滚动哨兵都真正生效，按用户方式逐屏下滚，观察 274 命中（> 单页 200）的完整路径。观察脚本是一次性临时文件，观察后已删除，不进入提交。实测结果：

- 首次 `search` 携带 `cursor: null`，续页携带后端返回的 `c200`，累计请求序列恰为 `[null, "c200"]`；
- 到达末页后再滚动不再产生新 `search` 调用（`isLast` 终止续页）；
- 标题在整个过程中保持 `274 首`，与已加载行数脱钩；
- 改动过的 `PlaylistsView.tsx` 在该观察下 25 个既有用例全绿。

改搜索词/排序不残留旧行、以及清空搜索恢复完整成员列表，由 §2 的「discards a continuation page from a superseded playlist search」与既有 `PlaylistsView.test.tsx` 用例覆盖（见 2.3 / 2.4）。

### 4.4 CR 阶段的发现

- **D8（已修）**：续页遇 `conflict` 时必须清空游标并重取首页，否则游标永久卡死。已加实现与「restarts the playlist search from the first page when a cursor goes stale」用例；该用例在回退实现后确认失败（其余 308 例仍绿），具备判别力。
- **已核对为无问题**：末页 `next_cursor` 为 `None`（`query.rs:140` 的 `if is_last { None }`），`loadMoreSearchResults` 的 `!cursor` 守卫能正确终止；`total_count` 在加入游标谓词之前统计（`query.rs:106` vs `:115`），标题计数不会随分页漂移；`loadMoreSearchResults` 闭包里的 `searching` 虽可能陈旧，但 `SongList` 自身在 `SongList.tsx:189` 有 `!loading` 守卫、且请求序号会丢弃重复响应，双触发无害（与 `useSongs` 同构）。
- **测试文件连带修改**：`PlaylistsView.test.tsx` 的 `vi.mock("../../bridge")` 原先只导出 `assetUrl`/`bridge`，新增 `instanceof BridgeError` 判断后会取到 `undefined` 而在既有用例中抛错，故为 mock 补了真实的 `BridgeError` 类与 `bridgeError()` 构造助手。
- **任务表 4.1/4.2** 在加入 D8 后重跑：typecheck 0 错误、lint 0 错误（6 个既有 warning，与本次改动文件无关）、309/309 通过。
