# Library Nav Counts — Tasks

## 1. Core：领域类型与 Port

- [x] 1.1 在 `crates/echo-core/src/domain/catalog.rs` 新增 `CatalogCounts { all: usize, favorites: usize, recent: usize }` 与 `RECENT_VIEW_LIMIT`，带说明三个字段语义的文档注释（`recent` 上限 100、其余为 active root 的 available 歌曲）。
- [x] 1.2 在 `crates/echo-core/src/application/ports.rs` 的 `CatalogQueryRepository` 新增 `fn counts(&self) -> Result<CatalogCounts, Error>`，文档说明「无活动根目录 → `Unavailable`」与计数语义。
- [x] 1.3 在 `crates/echo-core/src/application/catalog.rs` 的 `CatalogQuery` 暴露 `counts()` 委托给 repo（验证 `cargo check -p echo-core` 通过）。

## 2. Core：SQLite 与内存实现

- [x] 2.1 在 `crates/echo-core/src/infrastructure/sqlite/query.rs` 新增 `catalog_counts(connection)`：两条 `COUNT(*)`（`library_root_uuid = ?1 AND availability = 'available'`，favorites 追加 `AND is_favorite = 1`），`recent = min(all, 100)`；无 active root 时返回 `Error::unavailable`。
- [x] 2.2 在 `infrastructure/sqlite/mod.rs` 的 `impl CatalogQueryRepository for SqliteDatabase` 中接线 `counts()` 到 `with_reader`。
- [x] 2.3 在 `application/testing/memory_database.rs` 的 `impl CatalogQueryRepository for MemoryDatabase` 中实现同语义 `counts()`（active root + `Available`，`recent` 截断 100）。
- [x] 2.4 在 `application/catalog.rs` 的单测中扩展：断言 pending-delete 与非活动根目录的歌曲不计入、`recent` 上限生效、两个实现语义一致（验证 `cargo test -p echo-core` 通过）。
- [x] 2.5 在 `infrastructure/sqlite/tests.rs` 补一条 SQLite 侧计数用例（验证真实 SQL 与内存实现给出相同数字）。

## 3. Desktop：DTO 与命令

- [x] 3.1 在 `crates/echo-desktop/src/ipc/dto.rs` 新增 `LibraryCountsDto`（`#[serde(rename_all = "camelCase")]`，字段 `all` / `favorites` / `recent`：`u64`），并加 `From<CatalogCounts>`。
- [x] 3.2 在 `crates/echo-desktop/src/runtime/services.rs` 新增 `library_counts()`，通过 `CatalogQuery::counts()` 取数并映射（验证 `cargo check -p echo-desktop` 通过）。
- [x] 3.3 在 `crates/echo-desktop/src/ipc/generate.rs` 追加 `LibraryCountsDto` 的 TS 生成，并运行 `pnpm generate:ipc` 重新生成 `apps/desktop/src/ipc/ipc-types.generated.ts`（验证生成测试 `committed_generated_file_is_in_sync_with_the_generator` 通过，不手工编辑生成文件）。
- [x] 3.4 在 `apps/desktop/src-tauri/src/commands.rs` 新增 `library_counts` 命令并在 `main.rs` 的 `invoke_handler` 注册（验证 `cargo check` 通过）。

## 4. 前端：计数 store 与渲染

- [x] 4.1 在 `apps/desktop/src/bridge/index.ts` 的 `BridgeCommandMap` 新增 `library_counts: () => unknown`。
- [x] 4.2 在 `features/library/coverPalette.ts` 将 `LibraryCounts` 扩展为 `all / favorites / recent`，把 `clearLibraryCounts` 更名为 `invalidateLibraryCounts`，并新增 `useLibraryCounts()` hook：主动调 `library_counts` 拉取、按 `revision` 重取、失败时保留上次已知值。
- [x] 4.3 让 `publishLibraryCount` 退化为乐观覆盖：收藏切换时立即 ±1，随后由失效重取校正（不阻塞 UI）。
- [x] 4.4 订阅 `library://status` 与 `songUpdates` 作为失效信号；`LibraryWorkspace` / `PlaylistsView` 在导入、删除、歌单变更后调 `invalidateLibraryCounts()`。
- [x] 4.5 在 `app/App.tsx` 为「最近添加」补计数，并为歌单项渲染既有 `memberCount`（验证渲染测试断言导航四项均显示计数）。
- [x] 4.6 新增/更新组件测试：断言「未打开过 favorites 视图时侧边栏已显示其计数」与「在全部歌曲视图点红心后 favorites 计数增加」（验证 `pnpm test` 通过）。

## 5. 验证与归档

- [x] 5.1 运行 `cargo fmt --check` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`（验证零告警）。
- [x] 5.2 运行 `cargo test -p echo-core -p echo-desktop`（验证 Core 与桌面层测试全绿）。
- [x] 5.3 运行前端 `tsc --noEmit`、`eslint` 与 `pnpm test`（验证类型、lint 与组件测试通过）。
- [x] 5.4 运行 `pnpm verify:task -- 6.1 6.8 10.6 10.9` 回归既有任务（验证目录视图、Repository 门控、歌曲行与歌单校验不漂移）。7.2 的 `git diff --exit-code` 在改动提交前必然非零，已改为验证生成器幂等（`shasum` 前后一致）+ `cargo test -p echo-desktop ipc::` 全绿。
- [ ] 5.5 按项目流程完成 `openspec sync` 同步 delta 到主规格并归档该 change。
