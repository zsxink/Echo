## Context

见 proposal.md「Why」。核心事实链（均从实际依赖源码核对，tauri 2.11.5 / tauri-runtime-wry 2.11.4 / wry 0.55.1）：

- 冷启动时 `odoc` Apple Event 经 `tao::AppState::open_urls` → `Event::Opened` → tauri `RunEvent::Opened`。**实测（2026-09-20，真机 macOS，探针构建）该回调在 `.setup()` 之前就已触发**——日志顺序为 `RunEvent::Opened` → `deliver_file_open` → `setup: supervisor managed`。本设计的初稿曾断言"事件只在主事件循环开始分发后到达、`set_callback` 在 `app.run()` 前已就位所以不丢"，该推论**被真机否决**：事件确实没丢，但**收事件的人（managed state）还没出生**。这是原缺陷的第一层根因，也是 D6 的由来。
- 第二层根因才是本变更的主体：`app.emit("app://file-open-request", ...)` 走 `manager.emit` → `listeners.emit_js_filter`，**只有 `js_event_listeners` 中存在该事件的已注册处理器时才注入 JS**（`tauri-2.11.5/src/event/listener.rs`）。前端 `listen` 在 React `useEffect` 挂载后才注册，冷启动时必然晚于 `RunEvent::Opened` 的投递 → 事件被静默丢弃，无缓存、无重试。
- 运行中实例双击走 `RunEvent::Opened` 同路径，但监听器早已注册 → 正常。

现有代码已有一个"supervisor 未就绪"的暂存队列（`StartupSupervisor.pending_opens` + `receive_file_open` + `on_ready` 排空，task 9.1），但它只覆盖"supervisor ready 之前"，不覆盖"supervisor ready 之后、前端监听器就绪之前"的窗口——正是 bug 所在。此外 `main.rs` 存在**两个** `StartupSupervisor` 实例（`.setup()` 内 `app.manage(StartupSupervisor::new())` 与 `wire_composition` 内部创建的另一个），`deliver_file_open` 用的是被 `manage` 的那个。

## Goals / Non-Goals

**Goals:**

- 保证任意时刻到达的 OS 文件打开请求都不被静默丢弃：要么立即投递到已就绪的前端监听器，要么暂存并在监听器就绪后按到达顺序补发。
- 以"前端已注册 file-open 监听器"作为投递就绪的第二道门（在 supervisor ready 之外），消除 `RunEvent::Opened` ↔ React `useEffect` 之间的竞态。
- 补自动化回归，证明"前端监听器注册前到达的路径最终被播放"，防止回归。
- 保持既有行为不变：运行中实例（监听器已就绪）仍立即投递；单实例 argv 转发路径、补发顺序语义均不得退化。

**Non-Goals:**

- 不改播放/临时项语义，不触 Core 领域逻辑。
- 不做事件通用缓存/重放机制（只作用于 `FILE_OPEN_REQUEST` 这条投递路径）。
- 不处理"用户明确关闭/退出时 OS 仍推送打开请求"这类无法表达为正常播放的场景（现有失败路径保持报告失败）。
- 不改动 `StartupSupervisor` 的 startup 门禁、写门禁或其他用途。

## Decisions

### D1. 用一个"前端就绪门 + 暂存区"承接 file-open 投递，而不是改 Supervisor 的 startup 阶段

`StartupSupervisor.pending_opens` 现在只服务于 pre-ready 窗口，且 `RunEvent::Opened` 总是在 supervisor ready 之后才到达——所以问题窗口是"ready 之后、前端监听器注册之前"。给 `StartupSupervisor` 的 `pending_opens` 增加一个"前端已就绪"门：

- 新增 `frontend_ready` 布尔状态（原子量），初始 `false`。
- `receive_file_open` 改为：`phase == Ready && frontend_ready` 时才立即返回 `Some(path)`；否则入队、返回 `None`（语义与现在一致地通过返回值区分"已投递/待补发"）。
- `RunEvent::Opened` 到达时若 frontend 未就绪 → 入队；前端注册监听器后调用新增命令 `file_open_frontend_ready`，壳层把 `frontend_ready` 置位并 `drain_pending_opens`，按序补发。

**为什么改 `StartupSupervisor` 而不是新建一个壳层队列：** `deliver_file_open` 已经依赖 `try_state::<StartupSupervisor>()` 做暂存，沿用同一结构只需加一个门与一个排空入口，不引入第二套队列/第二处幂等语义，也避免"两个 supervisor 实例"进一步分裂。

**备选方案（否决）：**
- *后端延迟 emit*：给 emit 包一个延时，赌前端能按时注册——竞态未消除，只是缩小窗口，不可接受。
- *前端轮询 bootstrap*：前端无法得知"补发何时发生"，需轮询或复杂握手，且仍要处理"轮询与接收之间又到达"的竞态。不如显式命令直接。
- *改用 `emit_to(EventTarget::app())` 给 Rust 侧监听器*：那是 Rust 侧事件，前端仍收不到，问题性质不变。

### D2. 前端在监听器注册成功后就绪信号，且"前端注册失败也补发一次兜底"

前端 `App.tsx` 的 `useEffect`：`subscribe` 返回的 Promise resolve 之后（即 `listen` 已注册、`js_event_listeners` 已含该事件），调用 `bridge.fireAndForget("file_open_frontend_ready")`。若 `listen` 本身拒绝（罕见），仍调用一次补发命令并 `reportBridgeFailure`——使排空至少被触发，宁可走到明确失败报告也不静默丢。

由于命令**必达**、重复调用**幂等**（`frontend_ready` 已置位则排空仅处理新入队的路径，`on_ready` 排空语义保证每个路径恰好投递一次），即使前端 effect 在 StrictMode 下双跑也无副作用。

### D3. 用一个真正单一的 Supervisor（消除现有双实例隐患）

`wire_composition` 内部创建的第二个 `StartupSupervisor`（`main.rs:273`）与 `.setup()` 里 `manage` 的那个是不同对象。虽然本次 bug 根因不在此，但两个实例并存会让"反正你查的是同一个"的假设极易再度踩空（例如后人以为 frontend_ready 门在 `AppServices.startup()` 那个实例上）。设计上把两者收敛为一个：

- `wire_composition` 通过参数接收 `.setup()` 已 `manage` 的那个 supervisor（或由 `wire_composition` 在需要 `AppServices` 持有之前 `try_state` 取回），`run_recovery` → `on_ready` 在其上执行，`AppServices` 也引用同一实例。

这条收敛与 bug 修复同属本变更，因为 D1 的 `frontend_ready` 门依赖"所有投递/排空触点都在同一个也许可不共享的实例上"的正确性——若不收敛，`AppServices.startup()` 与 `deliver_file_open` 的门状态会分叉。

> 说明：若实现时发现收敛会显著扩大改动面（如 `AppServices::with_runtime` 的构造协议被多处调用），可作为独立任务放在 D1 之后，并在 tasks.md 中显式标记"可单独提交、不阻塞 D1 的验收"——但门状态必须收敛到"单一事实"这一目标不能放弃。

### D4. 补发命令返回 `void`，不返回"本次补发了哪些路径"

前端不需要知道补发内容（`FILE_OPEN_REQUEST` 事件本身就是投递载体，前端通过同一监听器消费）。返回 `void` 与 `set_theme`/`set_close_behavior` 等 fire-and-forget 命令一致；补发结果是否失败通过已有 `reportBridgeFailure` 通道上报。这样生成的 `IpcCommandResultMap` 不加新类型，前端 `BridgeCommandArguments` 也只需一行 `file_open_frontend_ready: EmptyArgs`。

### D5. 回归测试分层

- **Rust 单元**（`runtime/mod.rs` tests）：直接驱动 `StartupSupervisor`——`receive_file_open` 在 frontend 未就绪时返回 `None`；置位 frontend_ready + 排空后返回全部路径、顺序保持不变；重复置位幂等；frontend 就绪后 `receive_file_open` 立即返回 `Some`。
- **后端注入/门禁**（如现有 file-open 场景机制）：如有 `scripts/verify/checks` 覆盖，按既有注入证明套件补一条，证明"前端就绪前的路径最终被播放"可被证伪。
- **前端** `App.test.tsx`：新增用例——渲染后 `listen`（测试桩）注册完成时调用 `file_open_frontend_ready`；随后 `__echoTest.emit` 仍被消费。既有用例足以锁定 emit 语义不回退。

### D6. supervisor 的创建时机必须早于 `Builder::build()`：由 `main()` 持有并捕获进闭包

D1–D3 都建立在"路径能进入队列"这个前提上。真机取证（2026-09-20）显示这个前提在冷启动时不成立：`RunEvent::Opened` 早于 `.setup()`，此时 `app.try_state::<Arc<StartupSupervisor>>()` 返回 `None`，`deliver_file_open` 走的是"丢弃"兜底分支——`frontend_ready` 门再严密也照不到这批路径。因此：

- `main()` 顶部 `let startup = Arc::new(StartupSupervisor::new());`，**在 `tauri::Builder::default()` 之前**创建，进程级唯一实例。
- `deliver_file_open(app, startup: &StartupSupervisor, path)` 改为**接收引用**而非自己 `try_state`；调用方（`RunEvent::Opened` 与单实例 argv 两条路径）各自把捕获的 `Arc` 传进去。捕获式所有权使"事件到达时 supervisor 是否已注册"这个时序问题从根上消失。
- `.setup()` 仍然 `app.manage(Arc::clone(&startup))`，使命令层（`file_open_frontend_ready`）能通过 `State<Arc<StartupSupervisor>>` 拿到**同一个**对象——这同时落实了 D3 的"单一事实"。
- 被否决的备选：在 `.setup()` 之前用别的机制（静态量、`OnceLock`）持有 supervisor——静态全局会引入与 `AppServices` 生命周期无关的可变全局态，且测试无法注入独立实例；捕获 `Arc` 天然满足"每进程一个、可被测试构造多个"。

**这条必须与 D1 同批发布**：只有 D6 保证路径能入队、D1 保证它在前端就绪后被排空，两者缺一都仍然是"双击只开应用不播放"。

### D7. 真机取证的判据（本变更的验收证据）

macOS 上无 devtools，且 `open --env/--stderr` 在此版本不向被启动进程传递环境变量。可用的取证手段（本次实际使用）：

- 临时探针：在 `main.rs` 加 `fn probe(msg)` 追加写固定文件，打完即撤（不在提交里留痕）。
- 窗口级截图：`CGWindowListCopyWindowInfo`（`/usr/bin/python3` + `ctypes` 调 CoreGraphics/CoreFoundation，无需 Quartz 模块）取窗口 id → `screencapture -l <id> -o`，可截到被其他窗口遮挡的 Echo 窗口。
- "真在播"判据不能只看"已加载"：用 ≥30s 的长音频，间隔数秒截两帧，进度点位移须与真实时间一致，且传输键处于暂停态（可暂停 = 正在播）。


## Risks / Trade-offs

- **双 supervisor 收敛若引入顺序依赖** → 收敛放在 D1 之后单独提交；`deliver_file_open` 先 `try_state` 取回已 manage 实例，不改取回路径，只改"谁持有、谁跑 recovery"。若 `AppServices` 构造协议改动过大，收敛任务可后置，但门状态收敛是硬要求。
- **前端 effect 在 StrictMode/重挂载下重复调用命令** → 命令幂等（`frontend_ready` 布尔已置位则排空无副作用），加注释说明。
- **`listen` 拒绝导致就绪信号丢失** → D2 的兜底"失败也补发一次"保证排空至少触发；残余的明确失败走既有报告通道，符合规格"或明确报告失败"。
- **补发瞬间又有新到达** → `receive_file_open` 与排空同锁；`on_ready`（排空）只取走当前已有路径，新到达在下一次投递/排空处理，无重复、无丢失。

## Migration Plan

- 无外部部署/数据迁移。壳层与运行时的 file-open 投递路径改动为内部行为变化，前端 + 后端同栈发布，无向前兼容问题。
- 回滚：还原 `main.rs` / `App.tsx` / `runtime/mod.rs` 改动即可回到现行为（含现缺陷），无残留状态。

## Open Questions

无。规格、方案与任务拆分已确定；可不改动规格地延迟到实现期的只有前端测试桩细节（D5 之三）。