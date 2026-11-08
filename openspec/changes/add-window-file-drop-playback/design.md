# Design

## Context

动机与范围见 [proposal.md](./proposal.md)；行为契约见 [specs/](./specs/)。本节只记录已核对的当前实现与约束，供实现阶段据此落地。

**既有链路**：`deliver_file_open`（`apps/desktop/src-tauri/src/file_open_gate.rs`）把单个已归一化绝对路径交给 `StartupSupervisor` 的 FIFO；前端 `App.tsx` 订阅 `app://file-open-request`，对 `paths` 数组**逐个**调用 `play_temporary_file`；`commands.rs` 对每个 path 判断是否在活动资料库内，分别走 `coordinator.play_context()` 或 `coordinator.play_temporary()`。二者都会 `replace_queue(...)` 并 `failed_round.clear()`，所以一次 N 个文件会重建 N 次队列，最终只剩最后一个。`file_open_frontend_ready` 是「监听器已注册」的第二个门，负责排空冷启动 FIFO。

**已核实的技术事实**（本 change 的关键纠正）：

- Tauri 2.11.6 / `tauri-utils` 中 `drag_drop_enabled` 默认 `true`，配置注释明确它是「used internally to generate `DragDropEvent`s」的 webview handler 开关，映射到 `WebviewBuilder::disable_drag_drop_handler`；把它设为 `false` **会禁用壳层的 `WindowEvent::DragDrop`**，只在需要前端原生 HTML5 DnD（尤其 Windows）时才该禁用。因此本 change 应显式声明 `dragDropEnabled: true` 以消除默认值漂移，而不是设为 `false`。#117 现状描述里「默认 webview 拖放拦截」的措辞据此修正：窗口无响应的原因是**没有任何投放处理代码**，不是事件被吞掉。
- `tauri_runtime::window::DragDropEvent` 提供 `Enter { paths, position }` / `Over { position }` / `Drop { paths, position }` / `Leave` 四个变体，`paths: Vec<PathBuf>` 足以同时驱动反馈层与投递，无需在 WebView 侧再挂一套监听。
- wry 的 macOS 实现从 `NSFilenamesPboardType` 取 `PathBuf`，Linux WebKitGTK 从拖放 URI 经 `path_buf_from_uri` 转 `PathBuf`；两者都已是本地路径形态。投放路径与 `normalize-os-file-open-paths` 的已解码绝对路径属于同一口径，前端不得再解码（`%20` 字面量、中文、空格）。
- `capabilities/main.json` 当前只有 `core:default`、`core:window:default` 与四个显式 window 权限；新增事件由壳层 `app.emit` 后端发出，前端订阅无需新增权限族。
- `SUPPORTED_EXTENSIONS` 是 Core `crates/echo-core/src/application/scan/mod.rs` 的 8 项矩阵，导入对话框过滤与扫描共用；投放过滤必须复用同一判定，不另写白名单。
**实施复核**：`cargo tree -p echo-app -i tauri` 指向 tauri 2.11.6 / tauri-runtime 2.11.3；`dragDropEnabled: true` 已显式写入两份窗口配置（Windows 配置除 `decorations` 外逐字段一致）。过滤复用 Core 已公开的 `echo_core::application::scan::is_supported_extension`（8 项矩阵，扩展名先小写化），而非导入对话框的 7 项 `AUDIO_DIALOG_EXTENSIONS`。另：重跑 `extract-prototype-css.mjs` 时发现生成物 `player.css` 的 `.queue-window` 规则在原型中缺失（既有漂移）；为不回归 `QueuePanel` 的窗口化列表，把该规则原样补回原型再生成，生成物净变化只落在 `shell.css` 的投放层样式。

- `scripts/verify/spec-scenarios.mjs` 的 `AREA_PREFIX` 是固定映射，场景 ID 按 spec 内 heading 顺序派生。新增场景必须**追加**在既有 requirement / capability 末尾，否则既有稳定 ID 会整体重排，`docs/traceability.md` 与 `manifest.json` 三方对账随即失败。
- IPC 契约由 `crates/echo-desktop/src/ipc/generate.rs` 生成 `apps/desktop/src/ipc/ipc-types.generated.ts`，`apps/desktop/src/bridge/index.ts` 手写类型与生成物配套；`pnpm --filter @echo/desktop generate:ipc` 重建生成物，禁止手改。

## Goals / Non-Goals

**Goals:**

- 用最小的跨层改动把「拖放」接成既有 file-open 通道的一个新入口，保持与双击打开 100% 同语义（Q3=A）。
- 一处改动同时修掉多文件只剩最后一个的既有缺陷，且不改变队列替换语义。
- 让投放反馈、拒绝、路径与到达时机都有可验证的规格场景，并在三平台一致。
- 保持 Core 不变、WebView 权限面不变、既有稳定场景 ID 不变。

**Non-Goals:**

- 不引入「播放 / 导入」双落点、不实现文件夹或扫描目录投放（Q1/Q2）。
- 不实现「追加到队尾」「下一首播放优先区」「可切回的临时队列」，不触碰 #5 的优先区去重与提升逻辑。
- 不新增 capability 目录、不新增第三方依赖、不改 SQLite schema 或远端协议。
- 不在本 change 顺手处理 #6/#9 的 open-file 状态机其它分支，也不做歌单拖拽排序。

## Decisions

### D1 — 采用方案 B（壳层窗口事件投递），方案 A 仅作为 Linux 不可用时的回退

`bootstrap.rs` 的构建链上注册 `on_window_event`，匹配 `WindowEvent::DragDrop`：

- `Enter` / `Over` → 发 `app://file-drop-state`（active）；`Leave` → inactive；
- `Drop { paths }` → 过滤后按 `paths` 原顺序一次性交给 `deliver_file_open` 的批量入口，同时发 inactive。

理由：Q3 要求「跟双击一样」，B 直接复用 `StartupSupervisor` FIFO、`file_open_frontend_ready` 双门、`record_gate_open` 与既有事件名，播放链路语义不产生第二套实现；`DragDropEvent` 本身已带 `Enter/Over/Leave`，D1 反馈层无需前端拖放 API。

**复核修订**：两条 OS 打开入口（单实例 argv 与 macOS `RunEvent::Opened`）原先把每个路径各自 emit 一个 `app://file-open-request`。只改前端消费方式时，运行中一次打开 N 个文件仍会产生 N 次队列替换，`DP-R07-S08` 依旧失败。因此两条入口一律改为一次 `deliver_file_opens`（路径仍逐个过 FIFO 门），删除无消费者的单路径 `deliver_file_open`，并同步 Gate 9.1 / 9.2 的结构断言、`manifest.json` 的任务描述与 `injection-governance.mjs` 的 `file-open-normalization/raw-url-payload` 注入锚点。`file-drop-config.mjs` 增加断言锁住这一点。**方案 A**（`getCurrentWebview().onDragDropEvent()` + 前端拿绝对路径调批量命令）被否决：它把绝对路径再送一次进 WebView、另建一条与 OS open 并行的投递路径，且 enter/leave 与 drop 分处两端易产生状态竞态；收益（反馈层位置信息）B 已具备，本需求不需要落点坐标。组合两套监听还会带来重复 Drop 投递，需要额外去重，不值得。

回退预案：若 Linux WebKitGTK 实测拿不到可用 `paths`（见「Risks / Trade-offs」），保留壳层事件驱动反馈层，仅把 `Drop` 的路径获取改用方案 A 的前端 `onDragDropEvent` 并调用同一条批量命令；**spec 的外部行为契约不变**，需回头更新本 design 与 tasks。

### D2 — 显式 `dragDropEnabled: true`，不改其语义

基础 `tauri.conf.json` 与 `tauri.windows.conf.json` 的主窗口定义都显式写出 `dragDropEnabled: true`（Windows 平台配置除 `decorations` 外仍与基础配置逐字段一致，遵守 `desktop-app-shell`「Windows 无边框窗口与自绘窗口控件」的既有约束）。设为 `false` 会关掉生成 `DragDropEvent` 的内部 handler，直接否定 D1；显式写出只为消除 Tauri 版本升级时默认值漂移，行为不变。

### D3 — 投放反馈事件只传状态，不传路径

新增壳→前端事件 `app://file-drop-state`，payload 是结构化状态 DTO（`{ active: boolean }`），不含任何文件路径；另有 `app://file-drop-rejection`（`{ message: string }`）承载可读的拒绝原因，同样不含路径（文件名不进事件与日志，符合 §2）。反馈层只需要「是否处于可投放状态」，把路径留在既有 `app://file-open-request` 一条通道里，避免反馈组件接触绝对路径、也避免第二处解码责任。前端用一个 `useEffect` 订阅两个事件并在卸载时清理（`CODE_STANDARDS.md` §6）；拒绝原因经既有 `toastStore` 呈现，不新增浮层类型。

### D4 — 一次事件 → 一条批量命令，替换逐 path 调用

新增 `play_opened_files(paths: Vec<String>)` 命令；`App.tsx` 的 `app://file-open-request` 订阅改为**一次**调用该命令，传入整个 `paths` 数组。移除 `play_temporary_file` 及其在 `bootstrap.rs` 命令表、`crates/echo-desktop/src/ipc/generate.rs`、`ipc-types.generated.ts`、`bridge/index.ts`、`App.test.tsx` 中的全部引用。

这属于 `CODE_STANDARDS.md` §6 定义的 IPC 接口变更：唯一消费者是本仓库前端，无外部兼容负担；保留旧命令作为薄包装只会留下无消费者导出（§9 要求清理）。生成物只经 `pnpm --filter @echo/desktop generate:ipc` 重建。

实现时两个文件打开命令（`play_opened_files` 与 `file_open_frontend_ready`）落在 `apps/desktop/src-tauri/src/commands_file_open.rs`：留在 `commands.rs` 会使其超过 §2.1 的 1000 行规模门禁（`check-scale.mjs` 直接拒绝），故按「一种抽象一个文件」拆出文件打开命令模块，`main.rs` 声明、`bootstrap.rs` 命令表引用新模块。同理批量队列测试落在 `player/coordinator/tests_opened_files.rs`。

### D5 — coordinator 一次建队列，用显式枚举承载混合身份

`coordinator` 新增一个批量入口（例如 `play_opened_paths(entries: Vec<OpenedPlayEntry>)`），`OpenedPlayEntry` 是平台层枚举：`Library(SongId)` / `Temporary(TemporaryPlay)`。命令层按输入顺序解析每个路径：活动库内 → `Library`，否则读取临时元数据 → `Temporary`。批量入口在**一次** `replace_queue` 中按序 push N 个 `QueueEntry`、`set_current(第一个)`、清 `failed_round`、`bump_revision()`，最后**只发一次**加载命令。

这样消除 N 次 `replace_queue` / N 次 `Load` 的竞态，也保持 `desktop-playback` 的队列投影约束（当前项置顶、每个 entry 独立 ID、身份不以路径为准）。`play_temporary` / `play_context` 保留给其它既有调用方；批量入口不复用它们逐个调用，而是共享同一套「替换队列 + 重置随机轮」语义。Core 不感知 `OpenedPlayEntry`，该类型留在 `echo-desktop` 播放器层（`docs/DESIGN.md`：播放队列与设备播放状态属于平台层）。

### D6 — 过滤与拒绝在壳层完成，口径来自 Core

`Drop` 时逐个判断 `path.is_dir()`（→ 文件夹拒绝）与扩展名是否命中 Core `SUPPORTED_EXTENSIONS`（→ 非音频拒绝）。判定应通过 `echo-desktop` 已有的服务层暴露该矩阵（或复用导入对话框已在用的同一 helper），不得在壳层另写常量表。过滤结果按 `paths` 原顺序保留，不排序、不去重——「按拖放顺序」由 OS 给出的数组顺序定义，三平台若出现顺序差异按实测记录（Risks）。拒绝通过既有 Toast / 反馈层文案给出可读原因，不弹模态对话框、不写入门。

### D7 — 分层、依赖方向与设计模式

```text
OS 拖放 → Tauri 壳层 (Presentation/Platform)
              ├─ app://file-drop-state ─→ React 投放反馈组件 (Presentation)
              └─ deliver_file_opens → app://file-open-request → play_opened_files (Platform)
                                              ↓
                                  echo-desktop coordinator (Application/平台播放状态机)
                                              ↓
                                  mpv 播放适配器 / Core 资料库查询（只读）
```

- 依赖仍由外向内：React → Tauri command/event → `echo-desktop` → `echo-core`；没有反向依赖，Core 不新增接口、不引入 UI/播放器类型。
- **Command / Event**：投放状态与文件路径都是「已发生的状态变化」用 event，播放请求用 command，沿用 `desktop-app-shell` 既有边界。
- **State Machine**：冷启动 FIFO 与「前端监听器就绪」双门沿用 `StartupSupervisor`，不新增散落布尔值；投放反馈层的 active/inactive 是纯展示态，不进入播放状态机。
- **Ports and Adapters**：壳层事件捕获是平台适配，Core 支持矩阵是唯一过滤事实来源。
- 反馈组件不注册进 `useOverlay` / `OverlayTier`（它不是可交互浮层，`pointer-events: none`、无焦点陷阱、不参与 Escape 逐层退出），因此不改变 `immersive-lyrics` 与 `desktop-app-shell` 的浮层退出语义。

### D8 — 不新建 capability，保护稳定场景 ID

三个 delta 都挂在既有 capability：`desktop-app-shell`（新增 requirement 追加在文件末尾）、`desktop-playback` 与 `safe-file-ingestion`（MODIFIED requirement 的新场景追加在该 requirement 末尾）。`AREA_PREFIX` 无需新增映射，既有 577 个场景 ID 不重排；新增场景在 `docs/traceability.md`、`scripts/verify/scenario-commands.mjs` 与生成物 `manifest.json` 中按尾部新 ID 登记。

## 复核修订（post-implementation review）

- **P0（已修）**：OS 打开入口逐 path emit，见 D1 的复核修订。修复后 `DP-R07-S08` 的证据同时覆盖壳层静态门禁与 coordinator 批量测试，不再是假覆盖。
- **P1（已修）**：`Enter` 原先对任何拖拽（含纯文件夹/非音频）都点亮「松手即播放」，与 D1 的「受支持的音频文件」和 D5 的可读拒绝冲突。现由 `hover_transition` 在 `Enter` 时用同一 `partition` 判定，`Over` 不改变状态，`Drop`/`Leave` 清除；新增 `unsupported_or_directory_only_drag_never_promises_playback` 回归测试。
- **运行时链路核实**：`tauri-runtime-wry` 对 `WebviewKind::WindowContent` 把拖放回调合成为 `SynthesizedWindowEvent::DragDrop` → `WindowEvent::DragDrop`，因此 `Builder::on_window_event` 确能收到；回调返回 `true`，wry 不执行 OS 默认拖放（不会把文件当导航打开）。`dragDropEnabled: true` 正是启用该回调的开关。
- **多临时项核实**：`snapshot_queue` 用 `filter_map` 剔除队列中**所有** `Temporary` 项，`import_current_temporary_file` 只作用于当前项，因此 N 个临时项仍不进会话、不进统计。
- **已知弱项（未改，见交付说明）**：部分场景证据为静态/间接覆盖（`DAS-R26-S04` 沉浸模式、`DAS-R27-S07` 只读库、`SFI-R07-S06` 不入库）；混合投放的拒绝反馈按类别汇总而非逐项列出；`play_opened_files` 中单个路径解析失败会中断整批。三平台真机证据仍待补。

## Risks / Trade-offs

- **[Linux WebKitGTK 拖放路径投递能力未经真机实测]** → 实施第 1 组先做三平台最小验证（含 `paths` 是否为本地绝对路径、多文件顺序、URI 百分号解码是否已由 wry 完成）；若 Linux 拿不到可用路径，按 D1 回退预案改用前端 `onDragDropEvent` 取路径，其余设计不变。这是本 change 唯一可能改变「捕获点」的技术未知，spec 契约不受影响。
- **[OS 不保证多文件投放顺序]** → 以 `Drop` 事件给出的 `paths` 数组顺序为唯一事实来源，不做排序或去重；三平台冒烟记录实际顺序，若某平台顺序与用户选择顺序不同，作为已知限制写进验证证据而非在代码里猜测。
- **[路径二次解码会损坏中文 / 含空格 / 字面 `%20` 文件名]** → 拖放路径与 OS open 共用一条已解码绝对路径约定；`App.tsx` 注释与回归测试沿用「不得二次 decode」断言，新增投放路径的等价用例。
- **[反馈层遮挡标题栏拖拽条与 Windows 自绘按钮]** → `pointer-events: none` + 既有 `data-tauri-drag-region` / `WindowControls` 命中测试；覆盖 immersive / 浮层同时打开的状态。
- **[IPC 接口变更与生成物漂移]** → 旧命令引用全量清理，`generate:ipc` 后跑 CI 的 IPC 漂移检查与 `pnpm --filter @echo/desktop typecheck`；`bridge` 手写类型与生成物一并核对。
- **[MODIFIED requirement 复制不全导致归档丢细节]** → delta 中完整保留原 requirement 文本与全部原场景，新内容以追加段落/场景表达；归档前 `openspec validate --strict` 与 diff 复核。
- **[Q3=A 的替换队列是破坏性操作]** → 这是用户明确选定的语义；design 不再引入保护逻辑，但 tasks 要求「替换当前队列与随机轮」有独立回归场景，防止实现时被误改成追加。
- **[资料库不可用时临时播放仍可发声]** → `play_opened_files` 与 `play_temporary_file` 一样不经写入门，实现时不得顺手加 `guard_writes`；由独立场景固化。

## Migration Plan

无数据迁移：不涉及 SQLite schema、music library 目录结构、远端协议或持久化会话格式。

接口变更清单（全部为仓库内部）：新增事件 `app://file-drop-state` 与 `app://file-drop-rejection`；新增命令 `play_opened_files`；移除命令 `play_temporary_file` 与壳层内部单路径投递函数 `deliver_file_open`；`app://file-open-request` 事件名与 payload 形状保持不变（`readonly string[]`），只把消费方式从 N 次调用改为 1 次。

回滚：恢复 `play_temporary_file` 与 `App.tsx` 的逐个调用、移除 `on_window_event` 投放处理与两个投放事件订阅、恢复两处窗口配置；无残留状态需要清理。由于 #4 的修复同时作用于系统双击打开，回滚也会恢复该缺陷，需在回滚说明中注明。

发布顺序：先完成三平台投放能力验证（任务组 1），再按壳层 → 播放层 → 前端 → 文档门禁的依赖顺序落地；最后跑全量门禁与三平台原生冒烟。归档前按 `AGENT.md` 同步 delta spec 到主规格并完成归档，再创建关联 #117 的 PR。
