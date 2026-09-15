## Why

`desktop-app-shell` 规格已要求首次启动引导选择资料库根目录、以及三平台同等的"目录选择"能力，但生产壳（`apps/desktop/src-tauri`）至今用 `AppServices::new` 的测试替身 `TestDialogs::cancelling()` 接线：点击"选择资料库目录"永远返回取消、无任何反应；"导入文件"与"在访达中显示"同样假装成功。这是规格已验收、实现未落地的缺陷。同时，欢迎/初始化界面因 CSS grid 区域错位只占用侧边栏列的一半屏幕。

## What Changes

- 在 `echo-desktop` 保留 `SystemDialogs` 契约与 `TestDialogs` 测试替身（不变）。
- 在 `apps/desktop/src-tauri` 新增真实 `TauriDialogs` 实现 `SystemDialogs`：经 `tauri-plugin-dialog` 的 Rust 侧 `DialogExt` API 弹系统原生目录/文件选择器，经 `tauri-plugin-opener` 实现 reveal-in-folder。
- `main.rs` 改为用 `AppServices::with_runtime(...)` 注入 `TauriDialogs`，注册两个官方插件。
- **capability 与安全姿态完全不变**：Rust 侧调用原生对话框不经过 WebView capability 系统，`capabilities/main.json` 与 `CapabilityPolicy` 的"最小权限集不含 dialog"断言原样保留，WebView 永不获得 `dialog:`/`fs:` 权限、永不接触文件系统路径。
- 修复欢迎/初始化界面（`ChooseRootView`/`LibraryStatusView`）在 grid 中的区域错位，使其占满工作区而非侧边栏列。
- 让 `choose_library_root`、`choose_and_import_files`、`reveal_song` 三个命令从"测试替身"切换为真实系统能力。

## Capabilities

### New Capabilities

（无。本 change 不引入新能力，全部落在既有 `desktop-app-shell`。）

### Modified Capabilities

- `desktop-app-shell`: 精确化"目录选择"实现方式的架构约束（系统原生对话框、Rust 桌面侧调用、WebView 零文件系统接触），并新增欢迎/初始化界面占满工作区布局的可观察行为。现有首次选择/取消/不可用目录场景保持可观察结果不变。

## Impact

- **代码**：`apps/desktop/src-tauri/src/main.rs`（composition root 接线）、`apps/desktop/src-tauri/Cargo.toml`（新增 `tauri-plugin-dialog`、`tauri-plugin-opener`）、新增 `apps/desktop/src-tauri/src/dialogs.rs`（`TauriDialogs`）、`apps/desktop/src/app/app.css` + `ChooseRootView.tsx`/`LibraryStatusView.tsx`（布局修复），以及对应验证脚本。
- **依赖**：`tauri-plugin-dialog`（本地缓存 2.7.x）、`tauri-plugin-opener`。
- **不改变**：`echo-core`、`echo-desktop` 领域层、capability 权限集、CSP、`SystemDialogs` 契约、前端 IPC 协议。
- **架构约束**：保持 `echo-desktop` 不依赖 Tauri；真实对话框适配器位于 shell 侧，符合 `dialogs.rs` 顶部注释的设计意图。
- **验证**：新增 shell 侧 layout/接线测试与 verify check，`pnpm verify:task` 回归既有任务不漂移。