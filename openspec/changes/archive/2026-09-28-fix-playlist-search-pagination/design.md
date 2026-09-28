## Context

见 `proposal.md` — Why。本节只记录约束方案选择的现状。

后端能力齐备：Tauri command `search` 接受 `cursor: Option<String>` 与 `limit: usize`（上限 1..500），返回 `PagedSongs { items, totalCount, nextCursor?, isLast }`。资料库视图的 `useSongs` 已经用 `cursorRef` + `nextCursor` + `onLoadMore` 实现了这套键集分页，歌单搜索只是没接。

缺口全在前端（Presentation 层）：

- `PlaylistsView.tsx:191-224` 的 `playlistsMemberSearch` 是**一次性**回调——传 `cursor: null, limit: 200`，把整个 `PagedSongs` 存进 `searched` state。它没有保存 `nextCursor` 的地方，也没有续页的入口。
- `PlaylistsView.tsx:566` 的 `onLoadMore={() => {}}` 是空实现。成员列表走 `playlist_members` 全量返回、不分页，所以这个空实现对非搜索态是合理的；但搜索态走后端分页接口，超出第一页的结果没有任何途径加载。
- `PlaylistsView.tsx:497,536` 的计数读 `searched.totalCount`（后端全量命中数），与只渲染 200 行的列表直接矛盾。

约束：Code Standards §6 要求 effect 声明完整依赖并清理订阅、异步结果写入状态前处理过期响应；§2.1 要求单文件不超过 500 行（`PlaylistsView.tsx` 现 726 行，已超软上限）。

## Goals / Non-Goals

**Goals:**

- 搜索态接入与 `useSongs` 同构的键集分页：`nextCursor` + `isLast` + 真实 `onLoadMore`。
- 条件（搜索词/排序/歌单）变化时重置游标与已加载分页；并发请求以单调序号裁决，迟到结果丢弃。
- 末页后停止续页。
- 非搜索态（成员列表）行为逐字节不变。

**Non-Goals:**

- 不改后端分页语义、`PagedSongs` 契约或生成式 IPC 类型（无 IPC 变更，无类型重生成）。
- 不把成员列表改成分页——`playlist_members` 全量返回是既有且有价值的语义（追加顺序、批量操作、失效成员计数都基于它）。
- 不改 `useSongs` 本身（资料库路径已正确，歌单路径不复用它）。
- 不改定位（`useLocateSong` / `locateSongId`）在搜索态的行为。

## Decisions

### D1. 在 `PlaylistsView` 内联一个分页状态机，不抽共享 hook

**选择**：把 `playlistsMemberSearch` 从「一次性回调」改成维护 `searched: PagedSongs` + `nextCursor` 的分页状态机，`onLoadMore` 在搜索态调用续页、非搜索态保持空操作。

**理由**：`useSongs` 已经是那个 hook，但它的契约（`SongQuery` 带 `view`/`root`/`readOnly`、返回 `page`/`reset`/`retry`/`patchSong`、订阅 `subscribeLibraryInvalidations`）是为资料库工作区设计的。歌单视图的成员列表来自 `playlist_members` 全量、不走 `useSongs` 的查询链路，把歌单视图整体迁到 `useSongs` 会连带改变非搜索态的取数路径、失效成员展示和批量操作的基数——远超 issue 范围。

抽取一个共享的「游标分页」hook 也被否决：它要同时迁走 `useSongs` 的既有实现，改动面从一个视图扩到两个视图 + 它们的全部测试，而本次的净收益只是避免约 30 行相似状态机。按 §3.2「公共抽象至少应有两个合理实现……不要只为包裹一次函数调用而增加一层接口」，本次只满足单一实现，不构成抽取理由。

**分层**：变更完全落在 Presentation 层，不新增跨层依赖，不绕过既有抽象（`search` command 与 `PagedSongs` 都是现成契约）。

**替代方案**：
- *把歌单视图整体改用 `useSongs`*：会把非搜索态的成员取数也换掉，牵连失效歌曲成员（`availability !== "available"`）与批量全选基数，风险与范围都不可接受。
- *改 spec 为「歌单搜索不分页」*：issue 的最后一条验收标准给了这个出口，但它要求「同步修订 spec 中关于分页规则的表述」。规格把分页写成需求是刻意的（大型 DJ 歌单搜索命中过千是真实场景），删掉它比实现它更偏离产品意图；而且 `library-experience` 的「分页歌曲列表显示匹配总数」明确要求搜索结果跨页时总数仍正确，反向收紧只会让两个能力更割裂。**不采用。**

### D2. 请求序号沿用现有 `searchRequest` ref，扩成对游标续页同样有效

`searchRequest` 已是单调递增计数器，`playlistsMemberSearch` 每次 `++searchRequest.current` 后在 `.then` 里比对 `id !== searchRequest.current` 丢弃迟到结果。续页走**同一个计数器**（每次续页也自增），因此：

- 搜索词变化、排序变化、切换歌单 → `playlistsMemberSearch` 被重建并触发 → 自增 → 旧续页结果作废。
- 并发续页 → 后发者自增 → 先发者作废，不会出现两页交叉写入。

这与 `useSongs` 的 `requests: Map<key, reqId>` 是同构的（用全局单调值代替按 key 的 map，因为歌单视图只有一条查询链，不需要按 key 并存多个请求）。**不引入新的失效机制。**

### D3. 游标存在 ref，页数据存在 state

`nextCursor` 放 `useRef`，`searched`（含 `items`）放 `useState`，与 `useSongs` 的 `cursorRef` / `setPage` 完全一致：游标不需要触发渲染，放 state 会多一次无谓的重渲染；`isLast` 随 `searched` 一起存，因为 `SongList` 已经读 `searched.isLast`。

### D4. `onLoadMore` 按态分发

`onLoadMore` 变成一个具名回调：搜索态且未到末页时续页，否则什么都不做。成员列表的 `isLast` 已经硬编码为 `true`（`PlaylistsView.tsx:549`），所以 `SongList` 在非搜索态根本不会触发它——空操作分支只是防御性的显式声明，不改变既有行为。

**这会顺带修好搜索态下的定位**：`SongList.tsx:133-154` 的定位 effect 在目标歌曲不在已加载行内时会调用宿主的 `onLoadMore()`，并在 `songs`/`loading` 变化后重跑。旧代码传空实现，所以搜索命中超过 200 条时，定位在末页命中之外就直接判定「不在此列表中」。接入真实续页后，「定位到正在播放的歌曲 / 播放歌曲不属于当前视图」两个场景在搜索态下按 `library-experience` 的「定位当前播放歌曲」如实生效——这是 D1 的副产品，不额外扩大范围。

### D5. 重置时清空 `searched`

搜索词/排序/歌单变化后的首次请求返回前，`searched` 保持旧值还是立即置 `null`？选择**立即置 `null`**（`setSearched(null)`），因为 `displayedSongs = searched ? searched.items : sortedMembers`——置 `null` 会让列表短暂回退到全量成员列表，把「不属于当前搜索条件」的歌曲闪出来。改为在请求期间保持 `searched` 但把 `items` 视为空，则需要一个额外的 `searchPending` 态。

**取舍**：这里真正的约束是**不能闪现非当前条件的行**。两个方案都能满足，代价不同。保持 `searched` 旧值（items 仍可见）会让用户看到旧搜索词的结果继续挂着——这在 150ms 级的本地查询里几乎不可见，且与资料库视图 `useSongs` 的行为一致（`useSongs` 在 key 变化时**也**保留旧 `page` 直到新结果返回，只在 `reset()` 显式清空）。因此选择**保持旧 `searched`，不置 `null`**——与 `useSongs` 一致，且避免额外的 `searchPending` 状态。只有搜索词被**清空**时才置 `null`（回到成员列表），这正是现有行为。

### D6. 竞态与末页的测试落点

组件测试（`PlaylistsView.test.tsx`）用既有 bridge mock 覆盖三个场景：跨页加载（首屏 `isLast: false` → 触发续页 → 断言追加后行数）、末页终止（`isLast: true` → 断言不再发起 `search`）、竞态失效（首屏请求延迟返回期间改搜索词 → 断言迟到结果未渲染）。不写 Rust 测试——本变更不触碰后端。

### D7. 可追溯性同步

新增场景需要三方对账一致：spec → `docs/traceability.md` → `tests/scenarios/<id>.yaml` + `scripts/verify/manifest.json`。`scripts/verify/spec-scenarios.mjs` 从 spec 派生 ID 集合，`reconcile-scenarios.mjs` 强制两者相等，因此顺序是：

1. 手工向 `docs/traceability.md` 追加新场景行（ID 沿用 `LE-R18-S05…` / `PM-R09-S03…` 递增，**不得复用旧 ID**）。
2. 手工向 `scripts/verify/scenario-commands.mjs` 追加每个新 ID 的验收命令（该文件是命令的唯一来源）。
3. `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `manifest.json` 与 `tests/scenarios/*.yaml`（生成物，禁止手工编辑；`manifest.json` 是 1 空格缩进，禁止 `json.dump(indent=2)`）。
4. `node scripts/verify/reconcile-scenarios.mjs` 与 `pnpm verify:scenario -- <新 ID>` 验证。

`prd-matrix.md` 由 `node scripts/verify/prd-matrix.mjs` 生成，同步重生成。

## Risks / Trade-offs

**[迟到结果在视觉上仍短暂可见]** → D2 保证了它不会**写入** state；旧 `searched` 的残留渲染与 `useSongs` 同构，且搜索走本地 SQLite，延迟在毫秒级。若后续实测可见，再引入 `searchPending` 显式清空。

**续页请求未防重入]** → `loadMore` 入口检查 `searching`（与 `useSongs` 的 `if (!cursorRef.current || loading) return` 同构）。`SongList` 的滚动哨兵可能在渲染期间多次触发，序号裁决最终保证只有一页落地，但防重入能减少无谓请求。

**[PlaylistsView.tsx 已 726 行，超过 §2.1 的 500 行软上限]** → 本次只做**状态机内聚**（`playlistsMemberSearch` 从 34 行增至约 60 行），净增约 30 行。分页状态机提取为同目录的 `usePlaylistSearch.ts` 会更符合 §2.1 的「文件增长至接近上限时应先拆分再扩展」，但这属于独立的重构 change（`simplify` 范畴），不在 issue #31 范围内。建议后续单独处理。

**[规格文本已要求分页，本次只补场景]** → `library-experience` 的「歌单内搜索」原文已写明「分页/排序规则」，所以用 `## MODIFIED Requirements` 提交完整块（含原文全部场景 + 新增分页场景），而不是 `ADDED` 新的 requirement——新增 requirement 会让同一行为出现两处规范来源。

### D8. 游标过期的续页失败要回到首屏，不能只报错（CR 阶段补入）

后端把游标绑定到资料库修订号：`sqlite/query.rs` 在 `if let Some(cursor)` 时比对 `library_roots.updated_at`，不一致即返回 `conflict`（"catalog changed; restart pagination"）。而**任何歌曲 upsert 都会推进该时间戳**（`statements.rs` 的 `UPDATE library_roots SET updated_at …`），所以两页之间发生一次导入、扫描或收藏切换，游标就会失效——这是常规使用路径，不是边缘情况。

若续页只在 `.catch` 里提示失败而保留游标，`searchCursor.current` 仍指向那个已被后端拒绝的 token，用户每次滚动触底都会以同一个过期游标再失败一次，**永远无法继续翻页**，只剩一条死路。因此 `conflict` 分支先清空游标再以当前搜索词重取首屏：首屏请求不带游标，后端不会对它做修订号比对（`query.rs:71` 的 guard 只在 `Some(cursor)` 时进入），所以这次重查不可能再次冲突，递归深度恒为 1。

非 `conflict` 错误仍走原有提示。`useSongs` 对同一情形给出「查询已过期，请重试」，本 change 采取等价但更强的处理：不是让用户手动重试，而是自动重查首屏。

**代价**：重查会把已加载的分页清空（首屏是整体替换而非追加），用户会回到列表顶部。对失效游标而言这是唯一正确的恢复方式——继续用旧游标不可能成功。

## Migration Plan

无迁移。前端行为修复，无 schema、IPC 契约、Core API 或生成代码变化；回滚即 revert 该提交。
