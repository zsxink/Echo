## Context

启动真实应用发现：欢迎界面点击"选择资料库目录"无反应。排查确认 `apps/desktop/src-tauri/src/main.rs:93` 使用 `AppServices::new(...)`，其（`services.rs` 注释*"a safe default until the shell wires its real picker"*）注入 `TestDialogs::cancelling()` 测试替身 —— 目录/导入选择器永远返回取消，`reveal` 恒返回"已定位"。等于三个系统入口在真实壳全部是假实现。次要缺陷：欢迎/初始化页因 CSS grid 区域错位只占侧边栏列宽度的一半屏幕。

既有的 `SystemDialogs` trait（`crates/echo-desktop/src/platform/dialogs.rs`）已定义契约；`echo-desktop` 顶层注释明确*"stays verbatim Tauri-free"。真实适配器必须落在 shell 侧。

规格见 proposal.md（Why/What）+ specs（行为契约）。本设计只讲如何实现。

## Goals / Non-Goals

**Goals**
- 让 `choose_library_root` / `choose_and_import_files` / `reveal_song` 在真实壳使用系统原生能力。
- 保持 `echo-desktop` 与领域层（`echo-core`）零 Tauri 依赖。
- 保持 capability 安全姿态不变：WebView 不获得 `dialog:`/`fs:` 权限、不接触文件系统路径；`CapabilityPolicy` 断言原样通过。
- 修复欢迎/初始化页半屏布局。
- 三个系统入口全部接线，不留假实现。

**Non-Goals**
- 不改动既有 `SystemDialogs` 契约、`TestDialogs` 测试替身、前端 IPC 协议、`echo-core`/`echo-desktop` 领域逻辑。
- 不新增 WebView 侧文件系统/对话框能力。
- 不做导入/删除/扫描等功能本身的扩展。
- 不实现或修改三平台安装包分发（9.8 是另一 change）。

## Decisions

### D1: 接线位置 —— shell 侧 `TauriDialogs`

`echo-desktop` 保持 Tauri-free，真实适配器放 `apps/desktop/src-tauri/src/`。

- **选择**：新增 `TauriDialogs`（结构上持有 `tauri::AppHandle`，实现 `SystemDialogs`）。`main.rs` 由 `AppServices::new(...)` 改为 `AppServices::with_runtime(..., Arc::new(TauriDialogs), ...)`。
- **为什么优于替代**：
  - 备选 A（把 tauri 依赖加进 echo-desktop）：违反既有架构注释与分层，不可取。
  - 备选 B（前端用 `@tauri-apps/plugin-dialog` JS API）：会让 WebView 获得 `dialog:` capability，突破 7.7 的"无 dialog 权限"安全姿态，且破坏"WebView 零文件系统接触"设计。放弃。

### D2: 原生对话框来源 —— `tauri-plugin-dialog` 的 Rust 侧 `DialogExt`

官方插件 `tauri-plugin-dialog` 的 capability permissions（`allow-open` 等）**只 gate 前端 JS 命令**；Rust 侧 `app.dialog().file().blocking_pick_folder()/blocking_pick_files()` 不经 capability 系统。因此**Rust 侧调用时 capability 无需加任何 dialog 权限**。

- `pick_library_directory` → `app.dialog().file().blocking_pick_folder()` → `Option<PathBuf>`（`None`=取消，映射 `Ok(None)`）。
- `pick_audio_files` → `app.dialog().file().blocking_pick_files(...)`（多选，音频过滤），按 `ImportSource`/`ImportSourceReader` 契约构造 `PickedImport`。
- 约束：`blocking_pick_*` 不能阻塞主/事件循环线程。命令层需在 `async_runtime::spawn_blocking` 上执行选择（Tauri command 默认跑主事件循环，需确认；必要时把 `choose_library_root` 等做成异步 command 或显式 spawn_blocking）。

### D3: reveal-in-folder —— `tauri-plugin-opener`

官方 `tauri_plugin_opener::reveal_item_in_dir` 跨平台封装（macOS reveal / Windows explorer 选中 / Linux 打开父目录），映射为既有 `BackendReveal`/`RevealTarget` 纯逻辑模块的输入，产出 `RevealOutcome`。

- 替代：直接在 shell 分发平台命令（`open -R`/`explorer /select`/`xdg-open`）。opener 插件更统一、官方维护、天然处理平台差异，故用它。不用 `Shell` 权限（WebView 侧仍无 shell 能力）。

### D4: 布局修复 —— grid 区域归属

`ChooseRootView`/`LibraryStatusView` 的容器 `workspace-empty` 作为 `grid` 直接子项，未声明 `grid-area`，落入首个区域 `sidebar`（220px 列）。给 `.workspace-empty` 补 `grid-area: workspace`，让初始化/状态页占满工作区列。

### D5: 验证策略

- `TauriDialogs` 依赖 `AppHandle`，无法单测原生调用。把"选择 → 结果转换"拆成可注入的纯函数（dialog 闭包 trait），复用 `TestDialogs` 已证明的契约。
- shell 侧新增布局测试（断言未配置/不可用时 `.workspace-empty` 处于 workspace 网格区域、占满宽度）。
- 新增 verify check，并把既有 verify:task 回归跑一遍确保 capability 断言与 7.7 测试不漂移。

## Risks / Trade-offs

- [R] `blocking_pick_*` 阻塞事件循环导致 UI 卡死 → Mitigation：命令在 `spawn_blocking` 线程执行原生选择；彻底验证不阻塞。
- [R] 新增插件打破 7.7/`CapabilityPolicy` 的"零 dialog"漂移断言 → Mitigation：**capability 不加任何 dialog/fs 权限**（Rust 侧调用不需它）；跑 verify:task 7.7 确认 CapabilityPolicy 测试原样通过；本 change 明确不改 `capabilities/main.json`。
- [R] `tauri-plugin-opener` 平台差异（Linux 某文件管理器不能选中行）→ Mitigation：沿用 `BackendReveal::NotLocatable` → `RevealOutcome` 映射，UI 已支持降级显示相对路径。
- [R] `reveal_song` 在未接线前 UI 已依赖 `revealed` 字段（Task 10.8 已验收其前端行为）→ Mitigation：`TauriDialogs::reveal` 返回真实后端结果，前端已 handle `revealed=false`。

## Migration Plan

- 不涉及数据迁移。
- 回滚：`main.rs` 改回 `AppServices::new` + 去掉两个插件依赖即回到当前 stub 状态；capability/领域层零改动。
- 发布顺序：本 change 只影响桌面壳；无跨端依赖。

## Open Questions

无。D2 中"command 是否需 spawn_blocking"为任务级细节，实现时依 Tauri 版本确认即可，不影响规格/方案/任务拆分。