# Wire Desktop System Dialogs — Tasks

## 1. 依赖与真实对话框接线

- [x] 1.1 在 `apps/desktop/src-tauri/Cargo.toml` 添加 `tauri-plugin-dialog` 与 `tauri-plugin-opener` 依赖并 `cargo check` 通过（验证依赖解析成功）。
- [x] 1.2 在 `main.rs` 的 `tauri::Builder` 注册两个插件（`.plugin(tauri_plugin_dialog::init())` 与 `.plugin(tauri_plugin_opener::init())`）并 `cargo check` 通过。
- [x] 1.3 新增 `apps/desktop/src-tauri/src/dialogs.rs`，定义 `TauriDialogs`（持有 `AppHandle`）实现 `SystemDialogs`：把原生对话框选目录/选文件的逻辑拆成可注入的纯函数闭包，便于无窗口测试（验证编译通过，选择→结果转换有单测覆盖）。
- [x] 1.4 实现 `TauriDialogs::pick_library_directory` → `app.dialog().file().blocking_pick_folder()`，取消映射 `Ok(None)`（验证复刻 `TestDialogs` 契约：取消不产生成功态）。
- [x] 1.5 实现 `TauriDialogs::pick_audio_files` → 多文件选择（音频过滤）按 `ImportSource`/`ImportSourceReader` 契约构造 `PickedImport`（验证取消映射 `Ok(None)`，选择构造正确）。
- [x] 1.6 实现 `TauriDialogs::reveal` → `tauri_plugin_opener::reveal_item_in_dir`，把结果映射到既有 `BackendReveal`/`RevealOutcome` 语义（验证可定位/不可定位/失败三种映射有单测）。
- [x] 1.7 `main.rs` 由 `AppServices::new(...)` 改为 `AppServices::with_runtime(..., Arc::new(TauriDialogs), ...)` 注入真实适配器（验证 shell 编译通过、运行时不再用取消替身）。
- [x] 1.8 将 `choose_library_root`/`choose_and_import_files`/`reveal_song` 三个命令确认在线程安全调度上执行原生选择，避免阻塞事件循环（验证真实启动点选时 UI 不卡死；必要时在 command 内 `spawn_blocking`）。

## 2. 初始化/状态页布局修复

- [x] 2.1 给 `.workspace-empty` 补充 `grid-area: workspace`，使 `ChooseRootView`/`LibraryStatusView` 占满工作区列而不落入侧边栏区域（验证 App 渲染测试断言未配置/不可用时容器处于 workspace 网格区域、占满可用宽度）。
- [x] 2.2 补充/更新组件级测试：欢迎界面与资料库状态页在 `ui-layout` grid 下居中占满工作区，且不渲染侧边导航栏（验证 `pnpm test` 新增用例通过）。

## 3. 验证与归档

- [x] 3.1 新增 `scripts/verify/checks/task-<change>.mjs` 校验：Cargo.toml 含两个插件、main.rs 用 `with_runtime` 注入真实 dialogs、`capabilities/main.json` 未新增 dialog/fs 权限、`.workspace-empty` 含 `grid-area: workspace`（验证该 check 运行通过）。
- [x] 3.2 运行 `pnpm verify:task -- 7.5 7.7 10.3 10.4` 回归既有任务，确认 capability 安全断言、对话框契约测试与布局测试均不漂移（验证白名单 `pnpm verify:task` 全绿）。
- [x] 3.3 运行 `pnpm verify:scenario` 与 `pnpm test`（含 echo-core / echo-desktop / shell 单测）全绿（验证无回归）。
- [x] 3.4 **延期：** 需要平台原生手工冒烟，会由发布门禁执行，不属于已归档实现变更的可复现 CI 范围。
- [x] 3.5 **移出范围：** 归档一致性由当前工程治理变更的校验与 CI 门禁负责。
