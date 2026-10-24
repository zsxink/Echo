# Proposal

## Why

macOS 上 Echo 目前显示 Tauri 的默认英文菜单（`Echo` / `File` / `Edit` / `View` / `Window` / `Help`），与中文界面和产品定位不符，且不承载任何 Echo 功能：导入、选择资料库、重新扫描、设置、播放控制、播放模式、视图切换、搜索都只能从 WebView 界面里操作；`Help` 还是空菜单。同时，菜单栏状态项的右键菜单只有「显示窗口 / 退出」两项，与状态项承载的控制能力不对齐（issue #73）。

## What Changes

- macOS 用自建中文应用菜单替换 Tauri 默认菜单：`Echo`（关于/设置…/服务/隐藏/退出）、`文件`（导入/选择资料库目录/重新扫描/关闭窗口）、`编辑`（预置剪贴板项）、`播放`（播放控制+播放模式子菜单）、`视图`（六个视图入口+搜索+全屏）、`窗口`（预置窗口项）、`帮助`（GitHub 仓库/报告问题）。
- 播放类菜单命令走既有 `StatusMenuSink` → `PlaybackCoordinator` 权威通道（扩展 sink 支持的命令集）；导航/导入/搜索/设置通过 `menu://action` 事件交给前端既有动作；退出/帮助链接由壳层直接处理。
- 「播放/暂停」标签与「播放模式」勾选态由 `spawn_macos_status_row_snapshot_refresh` 同一条快照流驱动；「视图」勾选态由前端上报当前 `libraryView` 同步。
- 菜单栏状态项右键菜单从「显示窗口/退出」扩展为「显示窗口、导入音乐文件…、设置…、退出」。
- 设置页快捷键表与 `docs/interface-terminology.md` 补充菜单加速键说明。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-app-shell`: 新增要求「macOS 原生应用菜单与状态项右键菜单」（仅 macOS；Windows/Linux 不做菜单栏、托盘维持现状）。

## Impact

- 影响 `apps/desktop/src-tauri/`（新增 `app_menu.rs` 决策层与 macOS 菜单安装、`bootstrap.rs`、`main.rs`、`commands.rs`、`macos_status_row.rs`）与 `apps/desktop/src/`（`App.tsx` 菜单事件订阅、`useAppShell` 视图上报、搜索聚焦 store、三个工作区搜索框、`SettingsView` 快捷键表）。
- 不新增 WebView capability：菜单在 Rust 侧构建，`capabilities/main.json` 维持 `core:default` 最小权限。
- 文档：`docs/PRODUCT.md` 补充 macOS 原生菜单能力；`docs/interface-terminology.md` 新增菜单术语；`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`docs/acceptance/PRD-matrix.md` 登记新场景；`docs/native-attestation-playbook.md` 扩展菜单验收判据。
- 非目标：Windows/Linux 不做应用菜单栏，也不改托盘（维持现有 7 项结构，任何改动不得影响其行为）；不改前端 `useGlobalPlayerHotkeys` 的裸键语义。
