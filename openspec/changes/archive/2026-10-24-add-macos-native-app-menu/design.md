# Design

## 决策层：`app_menu.rs` 对齐 `status_menu.rs`

新增 `apps/desktop/src-tauri/src/app_menu.rs`，复刻 `crates/echo-desktop/src/platform/status_menu.rs` 的决策/执行分离模式：

- 语义 id 常量集中定义（`MENU_IMPORT`、`MENU_SETTINGS`、`MENU_VIEW_ALL_SONGS`…）；传输项复用 `status_menu::MENU_PLAY_PAUSE/MENU_PREVIOUS/MENU_NEXT`，退出复用 `status_menu::MENU_QUIT`，避免两套 id。
- 纯函数 `menu_action(id) -> Option<MenuAction>` 把 id 映射为三分类：

```rust
enum MenuAction {
    Playback(PlaybackAction),   // Command(PlayerCommand) | SeekBy(f64) | VolumeBy(f64)
    Frontend(FrontendAction),   // Import | ChooseLibraryRoot | Rescan | Settings | Search | View(LibraryView)
    Shell(ShellAction),         // Quit | HelpRepository | HelpIssues
}
```

- 决策层不依赖 Tauri 类型，跨平台编译、`cargo test -p echo-app` 三平台都可跑单测；只有菜单构建/安装/状态刷新部分 `#[cfg(target_os = "macos")]`。
- 快进/快退、音量增减是相对量，`PlayerCommand::Seek/SetVolume` 是绝对量：决策层提供 `seek_target(Option<f64>, f64) -> f64`（`max(0, position+delta)`，缺省 0）与 `volume_step(f64, f64) -> f64`（clamp 到 `[0,1]`），语义与前端 `useGlobalPlayerHotkeys` 的 `seekRelative`/`volumeRelative` 一致；解析时从 `PlayerHandle` 协调器快照读取当前 position/volume。

## 执行路径

- **播放类**（播放/暂停、上一首、下一首、seek、音量、静音、播放模式）：全部走托管状态里的 `Arc<RuntimeStatusMenuSink>` → `PlaybackCoordinator`，与托盘/状态项共用同一条权威通道。`RuntimeStatusMenuSink::on_command` 扩展 `Seek/SetVolume/ToggleMute/SetMode` 四个命令（调用 `coordinator.seek/set_volume/toggle_mute/set_mode`，与 `commands.rs` 的命令层语义一致）；未组合完成前仍由 `NoopSink` 行为兜底（菜单存在但无效果）。
- **导航/导入/搜索/设置/资料库**：`app.emit("menu://action", payload)`，payload 为 snake_case 动作字符串（`import`、`settings`、`search`、`choose_library_root`、`rescan`、`view:all`…，视图后缀与前端 `LibraryViewKind` 完全一致）。前端在 `App.tsx` 用 `bridge.subscribe` 接收，转调既有 `runImport` / `setSettingsOpen` / `selectView` / 搜索聚焦请求 / `choose_library_root`+状态刷新 / `start_scan`+失效刷新（复用设置页「重新扫描」的调用序列：`start_scan` → `invalidateCovers` → `invalidateLibrary` → `status.refresh`）。
- **窗口生命周期**：关闭窗口用 `PredefinedMenuItem::close_window`（performClose 会经过既有 `CloseRequested` 拦截，尊重「保持后台运行」偏好）；「退出 Echo」为自定义项，复用 `commands::flush_player_session` + `app.exit(0)`（与托盘退出闭包一致）。
- **帮助链接**：壳层经 `tauri_plugin_opener` 打开 GitHub 仓库/issue 页，不经 WebView。

## 菜单构建（macOS 专属）

- 通过 `tauri::Builder::menu(...)` + `.on_menu_event(...)` 在 `build()` 期间安装，替换 `enable_macos_default_menu` 的默认菜单，启动过程不闪现英文菜单。Windows/Linux 构建路径完全不加这两个 builder 调用（`#[cfg]` 分支），行为不变。
- 菜单条目全部使用 `Submenu`（macOS 要求）；自建条目写死中文；`PredefinedMenuItem`（关于/服务/隐藏/隐藏其他/显示全部/关闭窗口/撤销/重做/剪切/复制/粘贴/全选/最小化/缩放/全屏）显式传入中文文案——muda 0.19 的 macOS 实现接受自定义 title，不传才是英文默认。
- 加速键只用 `Cmd` 组合（`Cmd+O`、`Cmd+,`、`Cmd+W`、`Cmd+Q`、`Cmd+1..6`、`Cmd+F` 及预置项平台默认），不占用 `useGlobalPlayerHotkeys` 的裸键（Space、`,`、`.`、←→、↑↓、M）。
- 应用菜单第一项标题 `Echo`；其余子菜单标题为中文（文件/编辑/播放/视图/窗口/帮助）。

## 状态同步

- 「播放/暂停」标签与「播放模式」勾选态：扩展 `spawn_macos_status_row_snapshot_refresh`（`main.rs`）为同一条快照流同时刷新状态项图标与应用菜单（play/pause 标签、三个播放模式勾选）；mode 取自协调器（`coord.mode()`），函数签名增加协调器参数。菜单句柄存于 `app_menu` 内的 `OnceLock`（Tauri 菜单句柄 `unsafe impl Send + Sync`，仅主线程调用 mutator，符合其安全契约），避免与 `macos_status_row` 出现两套状态源。
- 「视图」勾选态：新增最小 IPC 命令 `report_library_view(view)`；前端 `useAppShell` 在 `libraryView` 变化（含首帧）时 `fireAndForget` 上报，Rust 侧解析后切到主线程更新六个视图勾选项。未知视图值不勾选任何项。
- macOS 状态项右键菜单：`macos_status_row::install` 增加一个 `frontend_action: Arc<dyn Fn(&str)>` 回调，右键菜单新增「导入音乐文件…」「设置…」两项，经回调转发到与主菜单相同的 `menu://action` 发射函数，保持单一决策层。

## 权衡与替代方案

- **不用前端 `@tauri-apps/api/menu` 建菜单**：需要给 WebView 增加 menu capability，破坏 `capabilities/main.json` 刻意维持的 `core:default` 最小权限设计；Rust 侧构建零新增权限。
- **不用 `app.set_menu` 在 setup 里替换**：默认菜单会在启动瞬间可见后又被替换；builder 菜单回调在窗口创建前生效。
- **播放模式菜单设为直接选择而非循环**：菜单是可见命名入口（与播放栏循环按钮不同），必须能一步选中目标模式并显示勾选态，所以 `menu_action` 映射为 `SetMode(mode)` 而非复用 `player_control mode:next` 语义。
- **视图上报用命令而非 JS→Rust 事件**：前端→壳层的既有通道就是 invoke 命令；`emit` 需要额外事件权限且无既有先例。

## 跨模块影响

- `echo-desktop` 不改（sink trait 不动，只扩展壳层 `RuntimeStatusMenuSink` 的实现）。
- Core 无改动。
- Windows/Linux：托盘代码路径（`#[cfg(not(target_os = "macos"))]` 块）零改动；`app_menu` 决策层纯函数跨平台编译并通过单测，作为「行为不受影响」的可执行证据之一。
