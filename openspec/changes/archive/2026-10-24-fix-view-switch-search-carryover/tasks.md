## 1. 三处视图切换清空搜索

- [x] 1.1 `PlaylistsView.tsx`：在既有 `playlistId` 导航重置副作用（清选中态/批量模式）中加入 `setSearchText("")`；派生的 `searched`/`searching`/`searchCursor`/`searchRequest` 复位由既有搜索 effect 的空词分支完成，不另写复位代码
- [x] 1.2 `LibraryWorkspace.tsx`：在既有 `view` 重置副作用中加入 `setSearch("")`；确认 `useSongs` 的 reqId 会丢弃切换瞬间以「新视图 + 旧词」发出的请求
- [x] 1.3 `CollectionDirectory.tsx`：在既有 `kind`/`root` 重置副作用中加入 `setSearch("")`；确认 `directoryRequest`/`detailRequest` 代际计数覆盖迟到结果
- [x] 1.4 回归确认：切换视图后选中态/批量模式清空行为不变（既有 effect 语义原样保留，仅追加清空）

## 2. 三个视图的组件回归测试

- [x] 2.1 `PlaylistsView.test.tsx` 新增「clears the search when switching to another playlist」：pl-1 搜索「晴天」命中过滤后 rerender 为 pl-2，断言搜索框为空、pl-2 完整成员可见、`search` 未以 `{playlist:"pl-2", query:"晴天"}` 的最终态过滤界面（以界面断言为准，切换瞬间的被弃请求不断言）
- [x] 2.2 `LibraryWorkspace.test.tsx` 新增「clears the search when switching between library views」：渲染真实 `App`，在全部歌曲输入搜索词后经侧边栏切到喜欢的音乐（或最近添加），断言搜索框为空、列表未被旧词过滤
- [x] 2.3 `CollectionDirectory.test.tsx` 新增「clears the search when switching between artist and album directories」：歌手目录输入搜索词过滤后 rerender `kind="album"`，断言搜索框为空、专辑条目未被旧词过滤；反向同理
- [x] 2.4 运行 `pnpm --filter @echo/desktop test -- --run src/features/playlists/PlaylistsView.test.tsx src/features/library/LibraryWorkspace.test.tsx src/features/library/CollectionDirectory.test.tsx`，三个新用例与既有用例全部通过

## 3. 可追溯性与规格同步

- [x] 3.1 手工向 `docs/traceability.md` 追加三行：`LE-R01-S05` 切换视图清空搜索、`LE-R01-S06` 切换歌手或专辑目录清空搜索、`PM-R09-S05` 歌单切换清空搜索；任务列写本 change 任务号（fix-view-switch-search-carryover 2.1–2.3），测试层 `React`，manifest 列指向 `tests/scenarios/<ID>.yaml`
- [x] 3.2 手工向 `scripts/verify/scenario-commands.mjs` 追加三个新 ID 的验收命令（`REACT_T` 过滤到 2.1–2.3 的用例名）
- [x] 3.3 运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*.yaml`，再运行无 `--write` 模式确认不再漂移
- [x] 3.4 手工更新 `docs/acceptance/PRD-matrix.md`：LE-R01-S05/S06 归入既有列出 LE-R01 各行的验收路径（A6/A13/A14，以各路径语义为准）并同步摘要「证明场景数」；PM-R09 不在 PRD 矩阵范围（既有 PM-R09 行缺席），如校验器要求则补
- [x] 3.5 运行 `node scripts/verify/reconcile-scenarios.mjs`（三方集合相等，基线 440 → 443）、`node scripts/verify/prd-matrix.mjs`（归属与摘要一致）、`pnpm verify:scenario -- LE-R01-S05 LE-R01-S06 PM-R09-S05` 逐项通过

## 4. 最终复核与归档

- [x] 4.1 运行 `pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test` 全量通过
- [x] 4.2 运行 `cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`（本次不改 Rust，应为无变化基线）；运行 `node_modules/.bin/openspec validate fix-view-switch-search-carryover --strict` 通过
- [x] 4.3 归档：delta spec 同步回主规格（`openspec` archive 流程），确认 `spec-scenarios` 编号与 traceability 一致后创建关联 issue #47 的 Pull Request

### 执行记录

- **narrow.test.tsx 连带重构（已修）**：「preserves search text, the current view and playback across a sidebar toggle」原用例先在全部歌曲输入搜索词、再点击喜欢的音乐，然后断言搜索词跨「视图切换 + 侧边栏开合」都保留——把 DAS-R08 的「开合/变窄不丢状态」与旧版「切视图保留搜索」耦合在一起。本 change 的 LE「资料库视图」修正后，前者仍成立、后者按规格反转。用例改为先切视图再输入搜索词，DAS-R02-S01（切换后标题/选中态）与 DAS-R08 的保留断言全部保留。
- **判别力验证**：临时还原三处组件修改后，三个新用例全部失败；恢复后全绿（378/378）。
- **门禁**：`verify:scenario LE-R01-S05 LE-R01-S06 PM-R09-S05` 通过（注意：本机 pnpm 11 会把 `--` 透传给 runner，报 `unknown scenario id: --`，直接 `pnpm verify:scenario <ID>` 即可）；`reconcile-scenarios` 443 = 443 = 443；`prd-matrix` 14 条路径 443 场景 1053 条归属边；`verify:governance` 33/33（首次运行暴露两个测试文件未过 prettier，`prettier --write` 后通过）。
- **归档时序**：traceability/命令映射/PRD 矩阵的登记与 `gen-scenario-manifests --write` 的最终重生成在 `openspec archive` 合入主规格之后完成（场景 ID 由主规格标题顺序派生，delta 合入前集合仍是 440）。
