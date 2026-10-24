# Design

## 1. 背景与目标

Windows 主窗口当前带原生标题栏（`decorations` 缺省为 `true`）；macOS 已用 `titleBarStyle: "Overlay"` + `hiddenTitle: true` + 前端 28px `--titlebar-inset` 安全区实现内容顶到窗口顶边。目标：Windows 去掉原生标题栏，自绘 最小化/最大化-还原/关闭，视觉与 macOS 一致；macOS/Linux 零变化。

## 2. 关键决策

### D1 配置落点：`tauri.windows.conf.json` 设 `decorations: false`，且写出完整窗口定义

- Tauri CLI 对平台配置按 JSON Merge Patch（RFC 7396）合并，**数组整体替换**。若 Windows 配置只写 `{"label":"main","decorations":false}`，主窗口将丢失宽高、min 尺寸与 macOS 字段，两平台窗口定义从此漂移。
- 因此 `tauri.windows.conf.json` 的 `app.windows[0]` 复制基础配置全部字段并追加 `"decorations": false`。
- 防漂移：`security.rs` 新增读取两份 JSON 的单测 `windows_conf_disables_native_decorations_without_drifting`——Windows 配置的窗口对象 = 基础配置窗口对象 + `decorations == false`，多字段/少字段/值不一致均失败。基础配置改动必须同步，测试即红。
- 不用 Rust 侧 `set_decorations(false)` 运行时开关：声明式配置可被 JSON 审查与漂移测试覆盖，且与 macOS overlay 的配置先例一致。

### D2 自绘控件：固定右上角悬浮，复用 `--titlebar-inset` 安全区

- 布局沿用 macOS 的既有机制：`.app { padding-top: var(--titlebar-inset) }` 已把内容压到安全区之下，控件 `position: fixed; top: 0; right: 0` 悬浮在安全区内，不与任何工作区交互元素重叠，不需要改 shell 网格。
- Windows 的 `--titlebar-inset` 由 `platform.ts` 初始化为 `36px`（控件高，接近 Win11 原生 caption 32px 并留出 4px 呼吸）；macOS 维持 28px；浏览器预览维持 0px（控件也不渲染）。
- 控件区与 `.titlebar-drag`（z-index 40）分离，z-index 45 保证悬浮其上；按钮为 `BUTTON` 元素，Tauri drag.js 对 clickable 元素自动豁免拖拽，控件下方条带仍是拖拽区。
- 双击最大化/还原：Tauri 注入的 drag.js 在 `mousedown detail===2` 时调用 `plugin:window|internal_toggle_maximize`，权限已含于 `core:window:default`，`.titlebar-drag` 行为零新增。

### D3 窗口命令经 bridge 封装（分层与 task 10.2）

- `bridge/index.ts` 新增窗口门面：`minimizeWindow` / `toggleMaximizeWindow` / `requestWindowClose` / `isWindowMaximized` / `onWindowResized`，内部经 `@tauri-apps/api/window` 的 `getCurrentWindow()` 调用。这是 `src/bridge/` 之外唯一被允许触碰 Tauri API 的边界（task-10.2 校验脚本约束），组件只见类型化函数。
- 关闭按钮调 `WebviewWindow.close()`：tao 在 Windows 发 `WM_CLOSE`、macOS `performClose:`，均进入既有 `RunEvent::WindowEvent::CloseRequested` 处理器，`CloseBehavior::Background` 时隐藏到托盘、`Exit` 时走 `ExitRequested` 清理——**与原生关闭完全同路径**，符合 issue「关闭按钮保持与原生一致，不加偏好特例」。
- `getCurrentWindow()` 惰性调用（不在模块顶层求值）：jsdom/浏览器预览没有 `__TAURI_INTERNALS__.metadata`，顶层求值会在导入期抛错；组件仅在平台为 Windows 时才触碰这些函数。

### D4 平台识别集中化：`documentElement.dataset.platform`

- 新模块 `src/app/platform.ts`：优先读 `documentElement.dataset.platform` 预置值（e2e/mock 以 `?platform=windows` 预置，prod 不预置）→ 无预置且无 `__TAURI_INTERNALS__` 时为 `web` → 有 WebView 时按 UA 判 `macos` / `windows` / `linux`。`applyPlatform()` 把结果写回 dataset，成为运行期单一事实来源；`main.tsx` 的 macOS UA 嗅探迁入此处（读 presence 不调 invoke，task 10.2 允许）。
- 组件与样式只看 dataset（`WindowControls` 挂载判断、e2e 断言），不再各自嗅探 UA。

### D5 权限面：三个显式命令，零特权族

- `MAIN_WINDOW_PERMISSIONS` 与 `capabilities/main.json` 同步追加 `core:window:allow-minimize`、`core:window:allow-toggle-maximize`、`core:window:allow-close`；`allow-internal-toggle-maximize` / `allow-is-maximized` 已含于 `core:window:default`，不重复登记。
- `CapabilityPolicy` 白名单不变：三个权限均非 `shell:/fs:/sql:/opener:/dialog:/process:/http:` 特权前缀；`committed_capability_grants_only_the_minimal_set` 漂移测试随常量更新自动锁定 capability 文件与常量一致。

### D6 样式落点：`app-extras.css`

- 窗口控件是实现层细节（原型 HTML 无此元素，浏览器预览不渲染），按既有分工写入 `app-extras.css`（同 `.titlebar-drag`、`--titlebar-inset` 先例），不触碰 `extract-prototype-css.mjs` 生成的四个提取层文件。
- 图标：`Icon.tsx` 按 1.8px 线型规范补 `winMinimize` / `winMaximize` / `winRestore` 三字形，关闭复用既有 `close`。

### D7 e2e mock 扩展

- `mock-bridge.ts`：安装 `__TAURI_INTERNALS__.metadata.currentWindow`（`getCurrentWindow()` 构造所需）；`invoke` 增加 `plugin:window|minimize|toggle_maximize|close|is_maximized` 处理并在 mock state 记录窗口动作；`installMockBridge` 按 `?platform=` 预置 `documentElement.dataset.platform`。
- 新 e2e 断言：`?platform=windows` 时 `.window-controls` 渲染三按钮、点击最大化图标在 maximize/restore 间切换、点击最小化/关闭被记录；无 `platform` 参数（web）时不渲染。

## 3. 分层与依赖方向

变更全部落在 Presentation/Platform 层：Tauri 配置（shell 装配）、`echo-desktop` 安全常量（既有边界）、React 组件与 bridge 门面。不触碰 `echo-core`、SQLite、同步与播放器；不新增跨层依赖。

## 4. 兼容性影响

- IPC 契约：无新 Tauri command / event（窗口插件命令不属于 `IpcCommandResultMap` 生成契约），`bridge/index.ts` 的命令表不变，IPC 生成物无漂移。
- Windows 细节：`shadow` 保持缺省 `true`（tao `undecorated_with_shadows` 保留 DWM 阴影与不可见缩放边，Aero Snap/贴边/拖到边缘缩放继续可用）；Win11 圆角由 DWM 对带阴影无装饰窗口默认保留，Win10 无阴影描边属已知平台差异，按验收场景在真机记录。最小尺寸（760×560）与任务栏缩略图不受 `decorations` 影响。
- macOS/Linux 回归：基础 `tauri.conf.json` 不动；Linux 维持原生装饰；`App.test.tsx` 的 `.titlebar-drag` 断言继续成立。
