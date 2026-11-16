# Tasks

## 1. 分支、基线与事实校核

- [x] 1.1 确认 issue #118 为 OPEN（`gh issue view 118 --repo zsxink/Echo --json number,title,state`），并确认已在任何生产代码改动之前从 `main` 创建独立分支 `feat/issue-118-collection-navigation-links`；禁止直接向 `main` 提交或推送。验证：`git branch --show-current` 输出该分支名，`git status --short` 只有本 change 的规划产物。
- [x] 1.2 记录治理基线：`node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/prd-matrix.mjs`、`node scripts/verify/check-scale.mjs`。验证：四条命令退出 0，场景基线为 609；`git diff --name-only` 不含生产代码。
- [x] 1.3 按 design D1–D6 重跑事实校核：`rg -n 'fromRowControl|track-artist|track-album' apps/desktop/src/features/library/SongRow.tsx`、`rg -n 'catalog_collections|catalog_collection_songs' apps/desktop/src/features/library/CollectionDirectory.tsx`、`rg -n 'player-track-card|song-meta' apps/desktop/src/features/player/PlayerBar.tsx apps/desktop/src/features/player/ImmersivePlayer.tsx`、`rg -n 'fn normalized_key|fn normalize_text' crates/echo-core/src/domain/text.rs`。验证：命中位置与 design「Context」一致；不一致时先修正规格再继续。

## 2. Core 分组身份

- [x] 2.1 在 `crates/echo-core/src/domain/catalog.rs` 把 `effective_artist` / `effective_album` 提升为公开 helper（缺失或空白元数据映射为“未知艺人”/“未知专辑”），并新增 `CollectionKeys { artist_key: Option<String>, album_key: Option<String> }` 与 `pub fn collection_keys(&Song) -> CollectionKeys`；key 仅在原始元数据 `trim()` 后非空时返回 `Some(normalized_key(...))`。验证：`cargo test -p echo-core --lib domain::catalog` 覆盖「NFKC/大小写/首尾与连续空白变体归入同一 key」「空白歌手与空白专辑返回 None」「同名专辑跨歌手得到相同 album_key」「歌手 key 随歌手名变化」。
- [x] 2.2 让 `crates/echo-core/src/application/catalog.rs` 删除私有 `effective_artist` / `effective_album`，改用领域 helper，并确认目录聚合与详情筛选的 key 规则与 `collection_keys` 共用同一实现。验证：`cargo test -p echo-core --lib application::catalog` 全部通过（既有聚合/排序行为不变），`rg -n 'fn effective_artist|fn effective_album' crates/echo-core/src/application/catalog.rs` 无输出。
- [x] 2.3 给 Core `SongDetail`（`crates/echo-core/src/application/detail.rs`）增加 `artist_key: Option<String>` / `album_key: Option<String>`，在 `build` 中用 `collection_keys` 填充，并更新该文件内所有构造点与测试夹具。验证：`cargo test -p echo-core --lib application::detail` 覆盖「已知歌手/专辑的 key 与 `collection_keys` 一致」「缺失元数据为 None」。
- [x] 2.4 运行 Core 静态门禁：`cargo fmt --all -- --check`、`cargo clippy -p echo-core --all-targets -- -D warnings`、`cargo test -p echo-core`。验证：三条命令退出 0。

## 3. 桌面 IPC DTO 与生成类型

- [x] 3.1 在 `crates/echo-desktop/src/ipc/dto.rs` 给 `SongView` 与 `SongDetailView` 增加可选 `artist_key` / `album_key`（serde camelCase + `skip_serializing_if = "Option::is_none"`），并在 `From<&Song>` / `From<&SongDetail>` 中用 Core `collection_keys` 填充；不新增持久化字段。验证：`cargo test -p echo-desktop --lib ipc::dto` 覆盖「已知元数据序列化出 `artistKey`/`albumKey`」「缺失元数据不出现这两个字段」「其余字段名与值不变」。
- [x] 3.2 重新生成 IPC 类型：`pnpm --dir apps/desktop generate:ipc`，确认 `apps/desktop/src/ipc/ipc-types.generated.ts` 中 `SongView`/`SongDetailView` 出现可选 `artistKey`/`albumKey`，禁止手改生成物。验证：再跑一次生成命令后 `git diff --exit-code apps/desktop/src/ipc/ipc-types.generated.ts` 无新增差异；`pnpm --dir apps/desktop typecheck` 退出 0。
- [x] 3.3 同步前端手写镜像类型：在 `apps/desktop/src/features/player/useSongDetail.ts` 的 `SongDetail` 接口增加可选 `artistKey` / `albumKey`，并确认 `useSongDetail` 原样保留后端返回值（不做展示名推导）。验证：`pnpm --dir apps/desktop typecheck` 与 `pnpm --dir apps/desktop test -- useSongDetail` 退出 0。

## 4. 壳层导航通道与 provider

- [x] 4.1 在 `apps/desktop/src/app/useAppShell.ts` 增加 `CollectionTarget` 判别联合（`artist` 只带 `artistKey`，`album` 只带 `albumKey`）、`CollectionSource { view, playlistId }`、`collectionTarget`、`collectionSource`、`openCollection`、`consumeCollectionTarget`、`returnToSource`：`openCollection` 仅在无待消费目标时记录来源，设置 `libraryView` 并清空 `activePlaylistId`；`consumeCollectionTarget` 只清目标；`returnToSource` 调用 `selectView(source.view, source.playlistId)`；`selectView` 同时清空目标与来源。验证：新增 `apps/desktop/src/app/useAppShell.test.tsx` 覆盖「跳转记录来源并切换视图」「快速连续跳转保留第一次来源」「消费后来源仍在、目标为空」「selectView 清除未消费目标」「直接进入目录无来源」。
- [x] 4.2 新增 `apps/desktop/src/features/library/collectionNavigation.tsx`：`CollectionNavigationProvider` 接收 `{ openArtist, openAlbum }`，`useOptionalCollectionNavigation()` 在无 provider 时返回 `null`。验证：新增 `collectionNavigation.test.tsx` 覆盖「provider 内拿到两个回调」「provider 外返回 null 且不抛错」。
- [x] 4.3 在 `apps/desktop/src/app/App.tsx` 顶层（`ShellNavProvider` 内、覆盖工作区与播放器）安装 `CollectionNavigationProvider`，值来自 `useAppShell().openCollection`；确认 `App.test.tsx` 既有渲染路径无需新增必填 props。验证：`pnpm --dir apps/desktop test -- App.test` 与 `pnpm --dir apps/desktop test -- accessibility` 退出 0。

## 5. 目录目标应用与返回来源

- [x] 5.1 给 `CollectionDirectory` 增加 `target`、`onTargetConsumed`、`onReturnToSource`、`returnLabel` props，并维护内部 `openedFromTarget`；`kind`/`root` 变化时重置。验证：`pnpm --dir apps/desktop typecheck` 退出 0，`LibraryWorkspace` 与 `PlaylistsView` 的调用点编译通过。
- [x] 5.2 实现目标应用：`target` 变化且 kind 匹配时清空目录搜索词并递增 `refreshEpoch` 触发一次新目录请求；在 `catalog_collections` 成功回调（请求代次校验之后）用最新 target ref 按 `artistKey`/`albumKey` 精确匹配，命中则 `setSelected(entry)`、`openedFromTarget = true`、`onTargetConsumed()`；未命中提示“未找到该歌手/专辑，可能已被移除”并消费；请求失败保留目标等待重试。验证：`pnpm --dir apps/desktop test -- CollectionDirectory` 覆盖「跳转命中直接打开详情」「搜索词非空时仍能命中」「目标不存在提示且停留目录」「请求失败后重试可命中」「目标只消费一次」。
- [x] 5.3 返回语义：用户从卡片手动打开条目时 `openedFromTarget = false`；返回按钮在 `openedFromTarget && onReturnToSource` 时调用来源返回，否则沿用 `setSelected(null)`；`aria-label` 使用 `returnLabel`。验证：`pnpm --dir apps/desktop test -- CollectionDirectory` 覆盖「链接进入的详情返回来来源视图」「目录卡片进入的详情返回聚合目录」「返回后再次进入不会重开目标」。
- [x] 5.4 控制文件规模：若 `CollectionDirectory.tsx` 因目标应用逻辑超过 1000 行，则把目标匹配/返回状态抽成同目录 hook 或纯函数模块；否则保持单文件。验证：`node scripts/verify/check-scale.mjs` 退出 0，`pnpm --dir apps/desktop test -- CollectionDirectory` 仍全绿。

## 6. 歌曲列表与歌曲详情链接

- [x] 6.1 修改 `SongRow`：`.track-artist` / `.track-album` 在存在对应 key 时渲染 `<button type="button" class="track-artist-link|track-album-link">`，`aria-label="打开歌手 X"` / `"打开专辑 X"`，点击 `stopPropagation` 并经 `useOptionalCollectionNavigation()` 导航；无 key 时保持纯文本（未知艺人文本保留、缺失专辑空白）。验证：`pnpm --dir apps/desktop test -- SongRow` 覆盖「点击歌手/专辑不触发行播放」「Enter/Space 与点击等价」「多选模式下选择集不变且不打开批量菜单」「无 key 不渲染按钮」「无 provider 时退化为纯文本」。
- [x] 6.2 修改 `SongDetail` 与 `SongMenu`：`<dd>` 中的歌手/专辑在存在 key 时渲染链接；`SongMenu` 把关闭详情与菜单的回调传给 `SongDetail`，激活链接时先关闭浮层再导航。验证：`pnpm --dir apps/desktop test -- SongMenu` 覆盖「详情对话框链接导航前关闭详情与菜单」「缺失 key 保持纯文本」。
- [x] 6.3 给 `apps/desktop/src/styles/library.css` 增加 `.track-artist-link` / `.track-album-link`（以及详情内链接）的按钮重置、`:hover`、`:focus-visible` 与 ellipsis，保持与既有 `.track-artist` 字号和颜色一致。验证：`pnpm --dir apps/desktop test -- SongList` 与 `pnpm --dir apps/desktop lint` 退出 0；`pnpm --dir apps/desktop format:check` 通过。
- [x] 6.4 确认全部歌曲列表入口共用 `SongRow` 的行为（`LibraryWorkspace`、`CollectionDirectory`、`PlaylistsView`），不遗漏独立渲染路径。验证：`rg -n 'SongRow' apps/desktop/src/features` 的每个调用方都有链接用例或由共享 `SongRow` 用例覆盖；`pnpm --dir apps/desktop test -- LibraryWorkspace` 与 `-- PlaylistsView` 退出 0。

## 7. 播放器表面链接

- [x] 7.1 重构 `PlayerBar` 的 `.player-track-card`：从单个 button 改为容器，封面+标题保留一个“展开播放器”按钮（保留 `aria-expanded`），歌手链接作为并列 button 放在文本列；有 key 时渲染链接，无 key 或临时播放项仍渲染原 subline 文本（艺人或时长）。验证：`pnpm --dir apps/desktop test -- PlayerBar` 覆盖「展开入口仍可打开沉浸式播放器」「歌手链接导航且不改变播放状态/当前歌曲/队列」「临时播放项无链接」「无 provider 时保持原文本」。
- [x] 7.2 修改 `ImmersivePlayer` 的 `.song-meta`：艺人/专辑在存在 key 时改为内联 button，激活时先 `setImmersiveOpen(false)`（并关闭歌词专注），再导航；缺失元数据保持纯文本。验证：`pnpm --dir apps/desktop test -- ImmersivePlayer` 覆盖「歌手/专辑链接关闭沉浸式后导航」「播放不中断、队列不变」「临时播放项与缺失元数据无链接」。
- [x] 7.3 给 `apps/desktop/src/styles/player.css`（必要时 `responsive.css`）补充播放器链接的按钮重置、hover/focus-visible 与窄屏不溢出规则。验证：`pnpm --dir apps/desktop test -- PlayerBar` 与 `-- ImmersivePlayer` 退出 0；`pnpm --dir apps/desktop format:check` 与 `lint` 通过。

## 8. 场景清单与追踪同步

- [x] 8.1 early-sync：把四个 delta spec 逐字同步到 `openspec/specs/` 对应主规格——`artist-album-browsing` 追加两条 ADDED 并替换 MODIFIED「从聚合条目浏览歌曲」；`library-experience` 末尾追加新 Requirement；`desktop-app-shell` 末尾追加新 Requirement；`immersive-lyrics` 在 R01/R03 各自末尾追加新 Scenario。不得中途插入场景。验证：`node scripts/verify/spec-scenarios.mjs --check` 输出场景总数为 621，且 `library-experience` 末条为 R23、`desktop-app-shell` 末条为 R29、`immersive-lyrics` 的 R01/R03 各有 4 条场景。
- [x] 8.2 在 `scripts/verify/scenario-commands.mjs` 为 12 条受追踪新场景补单条可执行命令：`LE-R23-S01..S06`、`DAS-R29-S01..S04`、`IL-R01-S04`、`IL-R03-S04`；`artist-album-browsing` 不在 `AREA_PREFIX` 注册表中，不登记场景命令。验证：`node scripts/verify/validate-scenario-commands.mjs` 退出 0；`node scripts/verify/run-scenario.mjs LE-R23-S01 LE-R23-S02 LE-R23-S03 LE-R23-S04 LE-R23-S05 LE-R23-S06 DAS-R29-S01 DAS-R29-S02 DAS-R29-S03 DAS-R29-S04 IL-R01-S04 IL-R03-S04` 全部通过（注意不要用 `pnpm verify:scenario -- <id>`，`--` 会被当成 id）。
- [x] 8.3 同步 `docs/traceability.md` 的 12 条新场景行（任务层/测试层沿用相邻行格式）。验证：`node scripts/verify/spec-scenarios.mjs --check` 报 spec 与 traceability 场景数相等（621）。
- [x] 8.4 重生成场景清单：`node scripts/verify/gen-scenario-manifests.mjs --write`，再跑一次不带 `--write` 确认 drift 为 0。验证：`scripts/verify/manifest.json` 与 `tests/scenarios|native` 只包含预期新增项；`node scripts/verify/reconcile-scenarios.mjs` 输出 621 = 621 = 621。
- [x] 8.5 按既有手工流程同步 `docs/prd-matrix.md`，把 12 条新场景挂到对应验收路径。验证：`node scripts/verify/prd-matrix.mjs` 输出与 621 个场景一致；`openspec validate add-collection-navigation-links --strict` 退出 0。

## 9. 集成验证、归档与交付

- [x] 9.1 全量静态与回归门禁：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-targets --all-features`、`pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop format:check`、`pnpm --dir apps/desktop test`、`node scripts/verify/check-scale.mjs`。验证：全部退出 0；新增/修改文件均低于 1000 行；`git status --short` 不含无关生成物。
- [x] 9.2 **独占**运行 `pnpm verify:governance`（不与 cargo、node 扫描或 rg 并行，避免 5 秒墙钟断言被干扰）。验证：整门退出 0、注入自证全部通过；失败只修本 change 引入的问题，既有 flake 如实记录。
- [x] 9.3 交叉检查实现与四个 delta spec：逐条确认每个 Scenario 都有对应实现或测试证据；重点核对「未知元数据不可点击」「多选模式仍可导航」「播放栏展开入口与歌手链接不嵌套」「目标只消费一次」「返回来源视图」。验证：核对结果记录在 PR 正文，缺口回填后再继续。
- [x] 9.4 确认 1–8 全部勾选、delta 已 early-sync、`openspec status --change add-collection-navigation-links` 显示任务完成。验证：`openspec validate add-collection-navigation-links --strict` 退出 0。
- [x] 9.5 运行 `openspec archive add-collection-navigation-links --yes`（delta 已同步，归档按幂等处理），确认 `openspec/changes/archive/` 下出现本 change 且主规格各 Requirement 只出现一次。验证：`openspec list --changes` 不再列出本 change；`rg -c '^### Requirement: 歌曲行与歌曲详情中的歌手/专辑链接' openspec/specs/library-experience/spec.md` 输出 `1`；归档后复跑 `node scripts/verify/spec-scenarios.mjs --check`、`node scripts/verify/reconcile-scenarios.mjs` 与 `pnpm verify:governance`（独占）均退出 0。
- [ ] 9.6 在独立分支以中文提交并推送，提交信息关联 #118（例如 `feat(library): 歌曲列表歌手/专辑链接直达聚合详情 (#118)`）。验证：`git status --short` 干净、`git branch --show-current` 为 `feat/issue-118-collection-navigation-links`、`git log -1 --oneline` 符合预期，且未直接向 `main` 推送。
- [ ] 9.7 用中文 PR 关联 #118：`gh pr create --base main --head feat/issue-118-collection-navigation-links --title ... --body-file ...`，正文包含变更摘要、Core/IPC 兼容性说明、场景与追踪同步结果、验证命令与结果、人工验收要点（各表面链接、返回来源、未知元数据置灰、多选模式），并以 `Closes #118` 关联。验证：`gh pr view --json url,title,body,state,headRefName` 返回正确内容；PR 未直接合并到 `main`（由维护者审阅合并）。
