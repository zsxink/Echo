# Tasks

## 1. 壳层配置与权限（Rust / Tauri）

- [x] 1.1 `apps/desktop/src-tauri/tauri.windows.conf.json`：补全 `app.windows[0]`（复制基础配置全部字段）并设 `"decorations": false`。
  - 验证：`cargo test -p echo-desktop --all-features platform::security`（15 用例通过，含新漂移测试）
- [x] 1.2 `crates/echo-desktop/src/platform/security.rs`：新增 `windows_conf_disables_native_decorations_without_drifting` 漂移测试（读两份 JSON，断言 Windows 窗口对象 = 基础对象 + `decorations:false`）；`MAIN_WINDOW_PERMISSIONS` 追加 `core:window:allow-minimize` / `allow-toggle-maximize` / `allow-close` 并更新文档注释。
  - 验证：`cargo test -p echo-desktop --all-features platform::security` + `cargo clippy -p echo-desktop --all-targets -- -D warnings`
  - 执行记录：`internal-toggle-maximize`（拖拽条双击）与 `is-maximized`（图标状态）已含于 `core:window:default`，无需重复登记。
- [x] 1.3 `apps/desktop/src-tauri/capabilities/main.json`：同步三个权限并扩展 description（Windows 无装饰窗口的自绘控件）。
  - 验证：`cargo test -p echo-desktop --all-features platform::security::tests::committed_capability_grants_only_the_minimal_set`

## 2. 前端：平台识别、bridge 门面、自绘控件

- [x] 2.1 新增 `src/app/platform.ts`：`AppPlatform` 类型、`detectPlatform()`（dataset 预置 → WebView presence → UA）、`applyPlatform()` 写回 `documentElement.dataset.platform`；附单测（预置优先、web 回退、三平台 UA）。
  - 验证：`pnpm --filter @echo/desktop test -- --run src/app/platform.test.ts`（6 用例通过）
- [x] 2.2 `src/bridge/index.ts` 新增窗口门面：`minimizeWindow` / `toggleMaximizeWindow` / `requestWindowClose` / `isWindowMaximized` / `onWindowResized`（惰性 `getCurrentWindow()`）；`main.tsx` 平台识别迁至 `platform.ts`，Windows 追加 `--titlebar-inset: 36px`。
  - 验证：`pnpm --filter @echo/desktop typecheck` + `node scripts/verify/checks/task-10.2.mjs`
  - 执行记录：task-10.2 校验脚本存在 Windows 路径分隔符缺陷（`bridge\index.ts` 不匹配 `bridge/` 前缀，本机常红而 CI 绿）与 `pnpm` shim 解析、ANSI 色码两处 spawn 问题，按最小改动修复（normalize 反斜杠、`shell: true`、匹配前剥离 ANSI）；不改变 macOS/Linux 行为。
- [x] 2.3 `Icon.tsx` 补 `winMinimize` / `winMaximize` / `winRestore` 字形；新增 `src/app/WindowControls.tsx`（仅 Windows 渲染三键、最大化图标随 `isMaximized` + `tauri://resize` 切换、`app-extras.css` 悬浮样式）；`App.tsx` 挂载；附组件单测。
  - 验证：`pnpm --filter @echo/desktop test -- --run src/app/WindowControls.test.tsx src/app/App.test.tsx`（11 用例通过）

## 3. e2e 与浏览器断言

- [x] 3.1 `e2e/mock-bridge.ts`：安装 `metadata.currentWindow`；`plugin:window|minimize|toggle_maximize|close|is_maximized` 处理并记录窗口动作（state 新增 `windowMaximized` / `windowActions`）；`installMockBridge` 按 `?platform=` 预置 dataset。
  - 验证：`pnpm --filter @echo/desktop typecheck`
- [x] 3.2 `e2e/a76-window-controls.mjs`（从 run-e2e.mjs 抽出以守住 1000 行 scale 门禁）：windows 强制下渲染 + 三键行为 + 图标切换；缺省 web 下不渲染。
  - 验证：`pnpm --filter @echo/desktop test:e2e`（#76 两条 ok；A9 歌词偏移与 audio-effects 截图两失败在 main 同样存在，属 in-flight change `align-immersive-player-and-playlist-directory` 与 Windows 临时路径环境问题，非本 change 引入）

## 4. 验收登记与整体验证

- [x] 4.1 `docs/traceability.md` 登记 DAS-R19-S01…S05；`scripts/verify/scenario-commands.mjs` 填命令（S01 ATTEST、S02/S04 REACT、S03/S05 cargo security 套件）；`gen-scenario-manifests.mjs --write` 重生成（460 = 460 = 460）；补 `docs/acceptance/PRD-matrix.md` 行并按 delta 同步主规格。
  - 验证：`node scripts/verify/reconcile-scenarios.mjs`（三方一致）+ `pnpm verify:governance`（除 coverage 与 injection proofs 外全部通过）
  - 执行记录：check-scale.mjs 同样存在 Windows 反斜杠 allowlist 失配（4 个白名单 trait 在本机误报超限），按同一方式修复。
- [x] 4.2 整体验证：`pnpm --filter @echo/desktop lint && typecheck && test --run && format:check` 全绿（44 文件 403 用例）；`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop -p echo-app --all-features`（420 + 107 用例）全绿；`node scripts/verify/checks/task-10.2.mjs` 通过。
  - 备注：coverage 门禁（task-12.8）在 CI ubuntu 上执行；本机 Windows 的 llvm-cov 统计（echo-core 86.51%）为平台差异，本 change 未触碰 echo-core。
- [x] 4.3 Windows 真机冒烟（本机 Win11，dev 构建模拟）：`decorations:false` 生效——无原生标题栏，内容自顶边 36px 安全区开始；WebView DOM 确认 `dataset.platform=windows`、`--titlebar-inset:36px`、三键（最小化/最大化/关闭）渲染；经真实 `plugin:window|toggle_maximize` 双击切换 aria-label 在 最大化↔还原 间翻转（真实 is_maximized + tauri://resize 驱动）；`plugin:window|minimize` 生效（is_minimized=true）；未授权的 `unminimize` 被权限系统正确拒绝（最小权限面生效）；单实例插件第二次启动合并到既有实例并聚焦。剩余人工项：拖拽贴边/Aero Snap/最小尺寸手感与 Win10 22H2 表现按 `tests/native/DAS-R19-S01.md` 由 operator 补 attestation。
- [x] 4.4 归档：`pnpm exec openspec validate remove-windows-native-titlebar --strict` 通过 → `openspec archive` → `pnpm exec openspec validate --archived` 通过。
