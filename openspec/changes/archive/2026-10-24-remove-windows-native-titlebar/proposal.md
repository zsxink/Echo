# Proposal

## Why

Windows 上 Echo 的主窗口仍保留原生系统标题栏（`tauri.conf.json` 的 `titleBarStyle: "Overlay"` + `hiddenTitle: true` 是 macOS 专属字段，在 Windows 上不生效；`decorations` 未设置，默认 `true`）。这条带「Echo」文字与 — □ × 按钮的系统横栏使用系统配色，与奶油色工作区在视觉上割裂；macOS 早已通过 overlay 标题栏让内容延伸到窗口顶边，Windows 是唯一未对齐的平台（issue #76）。

## What Changes

- `tauri.windows.conf.json` 为 Windows 平台补全主窗口定义并设 `decorations: false`（JSON Merge Patch 下数组整体替换，Windows 配置必须携带完整窗口字段；Rust 漂移测试锁定两份配置除 `decorations` 外不漂移）。macOS/Linux 配置不变。
- 前端新增仅 Windows 渲染的自绘窗口控件（最小化 / 最大化-还原 / 关闭），固定悬浮于窗口右上角、复用 `--titlebar-inset` 顶部安全区（Windows 与 macOS 同为非零值，浏览器预览仍为 0）；关闭按钮走 `CloseRequested → CloseBehavior` 原生路径，不新增退出特例。
- 窗口命令经 `bridge/index.ts` 统一封装（task 10.2 边界）；capability 与 `MAIN_WINDOW_PERMISSIONS` 同步新增 `core:window:allow-minimize` / `allow-toggle-maximize` / `allow-close` 三个显式权限（`internal-toggle-maximize`、`is-maximized` 已含于 `core:window:default`），不引入任何特权族。
- 平台识别集中到 `src/app/platform.ts`（`documentElement.dataset.platform` 单一事实来源，e2e 可经 `?platform=` 预置强制）；拖拽、双击最大化由既有 `.titlebar-drag`（`data-tauri-drag-region`）继续承担，无需新增拖拽代码。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-app-shell`: 新增要求「Windows 无边框窗口与自绘窗口控件」（仅 Windows；macOS overlay 标题栏与 Linux 原生装饰不变）。

## Impact

- 影响 `apps/desktop/src-tauri/tauri.windows.conf.json`（补全窗口定义）、`crates/echo-desktop/src/platform/security.rs`（`MAIN_WINDOW_PERMISSIONS` 与 Windows 配置漂移测试）、`apps/desktop/src-tauri/capabilities/main.json`（三个窗口命令权限）。
- 前端：新增 `src/app/platform.ts`、`src/app/WindowControls.tsx`；`main.tsx` 平台识别改经集中模块；`App.tsx` 挂载自绘控件；`bridge/index.ts` 封装窗口命令；`Icon.tsx` 补最小化/最大化/还原图标；样式归入 `app-extras.css`（实现层细节，不进原型提取层）。
- e2e：`mock-bridge` 支持 `?platform=` 强制与 `plugin:window|*` 命令，新增 Windows 渲染 / 非 Windows 不渲染两分支断言。
- 验收登记：`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`docs/acceptance/PRD-matrix.md` 增加 DAS-R19。
- 非目标：不做 Windows Snap Layouts 悬停菜单（系统对自绘按钮不提供，Aero Snap/贴边/拖拽缩放由系统边框继续提供）；不引入 liquid glass 材质（#74 另行处理）；不改关闭行为语义与设置项；macOS/Linux 不做任何窗口外观改动。
