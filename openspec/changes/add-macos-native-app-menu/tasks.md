# Tasks

## 1. 决策层与壳层（Rust）

- [ ] 1.1 新增 `apps/desktop/src-tauri/src/app_menu.rs`：语义 id 常量、`MenuAction` 三分类、`menu_action`/`seek_target`/`volume_step` 纯函数与 `menu://action` payload 常量；附单测（映射全覆盖、数学边界、payload 与前端 `LibraryViewKind` 一致、未知 id）。
  - 验证：`cargo test -p echo-app --lib app_menu`
- [ ] 1.2 `main.rs` 扩展 `RuntimeStatusMenuSink::on_command` 支持 `Seek/SetVolume/ToggleMute/SetMode`（对齐 `commands.rs` 语义），并新增 `report_library_view` 命令注册到 invoke_handler。
  - 验证：`cargo test -p echo-app --lib` + `cargo clippy -p echo-app -- -D warnings`
- [ ] 1.3 `app_menu.rs` macOS 区块：七个中文子菜单构建（自建中文条目 + 预置项传中文文案 + 加速键）、`OnceLock` 句柄存储、`refresh_playback_state`/`set_current_view` 状态刷新；`bootstrap.rs` 以 `Builder::menu` + `.on_menu_event` 安装并分发三类动作（播放→sink、前端→emit、壳层→退出/帮助链接）。
  - 验证：`cargo clippy -p echo-app -- -D warnings`（macOS 本机构建通过）
- [ ] 1.4 `main.rs` 扩展 `spawn_macos_status_row_snapshot_refresh` 为同一条快照流同时驱动状态项与应用菜单（play/pause 标签 + 播放模式勾选，mode 取自协调器）。
- [ ] 1.5 `macos_status_row.rs` 右键菜单补齐：`install` 增加 `frontend_action` 回调，右键菜单加「导入音乐文件…」「设置…」，复用主菜单同一条 emit 路径；更新既有单测。

## 2. 前端接线

- [ ] 2.1 新增 `src/app/searchFocus.ts` 聚焦请求 store；`LibraryWorkspace`、`CollectionDirectory`、`PlaylistsView` 的搜索输入订阅并聚焦。
  - 验证：`pnpm --filter @echo/desktop test -- --run searchFocus`
- [ ] 2.2 `App.tsx` 订阅 `menu://action`：import→`runImport`、settings→`setSettingsOpen(true)`、view:*→`selectView`、search→聚焦请求、choose_library_root/rescan→复用既有命令序列并刷新状态；附分发测试。
- [ ] 2.3 `useAppShell` 在 `libraryView` 变化（含首帧）时上报 `report_library_view`；`bridge/index.ts` 注册命令签名。
- [ ] 2.4 `SettingsView` 快捷键表补充菜单加速键行（导入/设置/搜索/视图切换）。
  - 验证：`pnpm --filter @echo/desktop test -- --run App` + `pnpm --filter @echo/desktop lint && pnpm --filter @echo/desktop typecheck`

## 3. 文档与验收登记

- [ ] 3.1 `docs/PRODUCT.md` Operating Context 补 macOS 原生应用菜单能力；`docs/interface-terminology.md` 新增应用菜单/播放菜单/视图菜单术语与加速键表。
- [ ] 3.2 `docs/traceability.md` 登记 DAS-R18 场景行；`scripts/verify/scenario-commands.mjs` 映射命令（代码级场景挂 cargo/vitest，原生场景挂 ATTEST）；`docs/native-attestation-playbook.md` 扩展菜单验收判据表。
- [ ] 3.3 归档 change 后 `gen-scenario-manifests.mjs --write` 重生成 manifest，补 `docs/acceptance/PRD-matrix.md` 归属行与摘要计数。
  - 验证：`pnpm verify:governance` + `pnpm verify:scenario -- <新场景>`

## 4. 整体验证

- [ ] 4.1 `pnpm verify`（lint/typecheck/单测/prettier）与 `cargo fmt --check`、`cargo clippy -D warnings` 全绿。
- [ ] 4.2 macOS 手工验收：真实点击菜单项核对七个场景判据（原生 attestation 证据记录）。
