# Tasks

## 1. 决策层与壳层（Rust）

- [x] 1.1 新增 `apps/desktop/src-tauri/src/app_menu.rs`：语义 id 常量、`MenuAction` 三分类、`menu_action`/`seek_target`/`volume_step` 纯函数与 `menu://action` payload 常量；附单测（映射全覆盖、数学边界、payload 与前端 `LibraryViewKind` 一致、未知 id）。
  - 验证：`cargo test -p echo-app --bin Echo app_menu`（10 用例通过）
- [x] 1.2 `main.rs` 扩展 `RuntimeStatusMenuSink::on_command` 支持 `Seek/SetVolume/ToggleMute/SetMode`（对齐 `commands.rs` 语义），并新增 `report_library_view` 命令注册到 invoke_handler。
  - 验证：`cargo test -p echo-app -p echo-desktop --all-targets` + `cargo clippy --all-targets -- -D warnings`
- [x] 1.3 `app_menu.rs` macOS 区块：七个中文子菜单构建（自建中文条目 + 预置项传中文文案 + 加速键）、`OnceLock` 句柄存储、`refresh_playback_state`/`set_current_view` 状态刷新；`bootstrap.rs` 以 `Builder::menu` + `.on_menu_event` 安装并分发三类动作（播放→sink、前端→emit、壳层→退出/帮助链接）。
  - 执行记录：窗口/帮助子菜单使用 Tauri 保留 id（`WINDOW_SUBMENU_ID`/`HELP_SUBMENU_ID`），`init_app_menu` 才会把它们注册为 NSApp 的窗口/帮助菜单；预置项必须显式传中文文案（muda 0.19 的 macOS 实现不传 text 时是英文默认，与 issue 预期一致）。
  - 验证：`cargo clippy -p echo-app --all-targets -- -D warnings`
- [x] 1.4 `main.rs` 扩展 `spawn_macos_status_row_snapshot_refresh` 为同一条快照流同时驱动状态项与应用菜单（play/pause 标签 + 播放模式勾选，mode 取自协调器）。
- [x] 1.5 `macos_status_row.rs` 右键菜单补齐：`install` 增加 `frontend_action` 回调，右键菜单加「导入音乐文件…」「设置…」，复用主菜单同一条 `dispatch_by_id` 路径。

## 2. 前端接线

- [x] 2.1 新增 `src/app/searchFocus.ts` 聚焦请求 store；`LibraryWorkspace`、`CollectionDirectory`、`PlaylistsView` 的搜索输入订阅并聚焦。
  - 验证：`pnpm --filter @echo/desktop test -- --run`（menuAction.test 覆盖聚焦）
- [x] 2.2 `App.tsx` 订阅 `menu://action`：import→`runImport`、settings→`setSettingsOpen(true)`、view:*→`selectView`、search→聚焦请求、choose_library_root/rescan→复用既有命令序列并刷新状态；附分发测试（`src/app/menuAction.test.tsx`，7 用例；已做判别力红验：禁用订阅后 6 例转红）。
- [x] 2.3 `useAppShell` 在 `libraryView` 变化（含首帧）时上报 `report_library_view`；`bridge/index.ts` 注册命令签名。
  - 执行记录：`report_library_view` 必须同时进入 Rust IPC 生成器（`crates/echo-desktop/src/ipc/generate.rs`）并重跑 `cargo run -p echo-desktop --bin echo-generate-ipc`——bridge 的 `AssertNever` 漂移门禁要求手写参数表与生成结果表键集完全一致，只在 TS 侧加签名会让全部命令类型塌缩为 `never`；`e2e/mock-bridge.ts` 的 Command 联合同理。
- [x] 2.4 `SettingsView` 快捷键表补充菜单加速键行（导入/设置/搜索/视图切换/关闭窗口/退出，仅 macOS 渲染）。
  - 验证：`pnpm --filter @echo/desktop lint && pnpm --filter @echo/desktop typecheck` + SettingsView 快捷键两用例

## 3. 文档与验收登记

- [x] 3.1 `docs/PRODUCT.md` Operating Context 补 macOS 原生应用菜单能力；`docs/interface-terminology.md` 新增原生应用菜单术语与加速键表。
- [x] 3.2 `docs/traceability.md` 登记 DAS-R18 场景行（S01–S08 原生、S09 cargo 单测）；`scripts/verify/scenario-commands.mjs` 映射命令；`docs/native-attestation-playbook.md` 扩展 §4/§5 菜单验收判据与逐步步骤。
- [x] 3.3 归档 change 后 `gen-scenario-manifests.mjs --write` 重生成 manifest，补 `docs/acceptance/PRD-matrix.md` 归属行与摘要计数。
  - 验证：`pnpm verify:governance` + `pnpm verify:scenario -- DAS-R18-S09`

## 4. 整体验证

- [x] 4.1 `pnpm verify`（lint/typecheck/单测/prettier）与 `cargo fmt --check`、`cargo clippy -D warnings` 全绿。
- [x] 4.2 macOS 冒烟：裸二进制 + `ECHO_GATE_DATA_DIR` 隔离启动，新菜单构建路径无崩溃、进程存活、SIGTERM 干净退出（真实菜单点击需辅助访问权限，按仓库惯例由操作者按 playbook §5 DAS-R18 步骤补 attestation 证据，同 DAS-R06/R15）。
