## 1. 运行时：前端就绪门 + 补发排空（core of D1）

- [x] 1.1 在 `crates/echo-desktop/src/runtime/mod.rs` 的 `StartupSupervisor` 增加 `frontend_ready: AtomicBool`（初始 `false`），新增 `mark_frontend_ready()`（置位 + 返回当前暂存路径并排空）或等价方法；在已有 `supervisor_sequence_resolves_gate_and_drains_pending_opens` 测试基础上补测试：frontend 未就绪时 `receive_file_open` 入队返回 `None`，frontend_ready 后排空返回全部路径且保持到达顺序，重复置位幂等，frontend 就绪后 `receive_file_open` 立即返回 `Some`。验证：`cargo test -p echo-desktop runtime` 通过。
- [x] 1.2 `receive_file_open` 改为仅当 `phase == Ready && frontend_ready` 时立即返回 `Some(path)`，否则入队返回 `None`；确认既有的 `pending_open_fifo_is_bounded_and_keeps_newest` 有界队列语义不被破坏（容量、新胜旧保持）。验证：`cargo test -p echo-desktop` 通过。
- [x] 1.3 收敛 `main.rs` 的双 `StartupSupervisor` 实例（D3）：`.setup()` 中 `app.manage(StartupSupervisor::new())` 创建的唯一实例由 `wire_composition` 取回用于 `run_recovery` → `on_ready`，并作为 `AppServices::with_runtime` 的 `startup` 参数；删去 `wire_composition` 内部第二实例。验证：`cargo build -p echo-desktop` 与 `cargo test -p echo-desktop` 通过；`AppServices::startup()` 与 `deliver_file_open` 的 `try_state` 指向同一对象（可用单元断言或构造层面保证）。

## 2. 壳层：新命令 + 投递门接线（D1/D2）

- [x] 2.1 新增 IPC 命令 `file_open_frontend_ready`（返回 `Result<(), IpcErrorDto>`，语义是"前端 file-open 监听器已就绪"），实现为取 `try_state::<StartupSupervisor>()` → `mark_frontend_ready()` → 对返回的路径 `app.emit(FILE_OPEN_REQUEST, paths)`；在 `commands.rs` 与 `main.rs` 的 `generate_handler!` 注册。验证：`cargo check -p echo-desktop`（含 src-tauri 目标）通过；命令在 `IpcCommandResultMap` 生成后可见。
- [x] 2.2 调整 `deliver_file_open`：保留 `try_state` 取 supervisor 的逻辑，但投递就绪判定改用 `receive_file_open` 的新语义——`phase == Ready && frontend_ready` 才立即 emit，否则静默入队（不再在未就绪时 emit 丢事件）；`RunEvent::Opened` 路径与单实例 argv 路径统一走它。验证：`cargo test`（src-tauri 或对应单元覆盖）通过，运行中实例行为不退化。
- [x] 2.3 确认 `drain_pending_opens` 的补发不丢、不重放：排空投递点唯一（对齐 design D1——setup 结束时 WebView 尚未加载、emit 必被静默丢弃，故 `on_ready` 不再排空，仅置位 phase；前端就绪后经 `file_open_frontend_ready` 命令排空）；`mark_frontend_ready` 幂等，同一 supervisor 上不重复消费（`on_ready` 只置位、`drain` 只取一次）。验证：`cargo test -p echo-desktop` 通过、`frontend_ready_gate_is_idempotent_and_never_replays` 证明不重放。
- [x] 2.4 运行 `cargo run -p echo-desktop --bin echo-generate-ipc` 重新生成 `apps/desktop/src/ipc/ipc-types.generated.ts`，确认 `IpcCommandResultMap` 出现 `readonly file_open_frontend_ready: void`（或等价），且 `git diff` 与既有一致风格。验证：`pnpm generate:ipc` 无报错，`git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts` 后重新生成成功。

## 3. 前端：监听器注册后拉起排空（D2）

- [x] 3.1 `apps/desktop/src/bridge/index.ts` 的 `BridgeCommandArguments` 增加 `file_open_frontend_ready: EmptyArgs`（与生成类型漂移门禁一致）。验证：`pnpm typecheck` 通过。
- [x] 3.2 `apps/desktop/src/app/App.tsx` 的 `useEffect`：在 `bridge.subscribe("app://file-open-request", …)` 的 Promise resolve 之后调用 `bridge.fireAndForget("file_open_frontend_ready")`；subscribe 拒绝时也调用一次并 `reportBridgeFailure`（D2 兜底）。补注释说明幂等性（StrictMode 双跑安全）。验证：`pnpm lint` 与 `pnpm typecheck` 通过。
- [x] 3.3 在 `apps/desktop/src/app/App.test.tsx` 增加用例：渲染后 `listen`（测试桩）注册完成时调用 `file_open_frontend_ready`；随后 `__echoTest.emit` 的既有消费断言不回退（复用现有用例基线）。验证：`pnpm test` 通过。

> 附注：`apps/desktop/e2e/mock-bridge.ts` 的 `Command` 联合类型新增 `file_open_frontend_ready`（drift 门禁要求每个命令被命名）；`apps/desktop/src/test/setup.ts` 的默认 invoke mock 对 `file_open_frontend_ready` 返回成功（该信号每次 App 挂载必发，与 `library_status` 同为启动默认，避免每个测试 opt-in）。两者是任务 3.x 触达的门禁/桩同步，非范围扩大。

## 4. 回归与治理接入

- [x] 4.1 核对 `scripts/verify/` 下是否有覆盖 file-open 冷启动场景的检查项（manifest 的 `scenarios` 与注入证明套件）；若有，按既有"注入证明"约定补一条可证伪的回归断言（证明"前端就绪前的路径最终被播放"可因条件被违反而失败），若无则记下并依赖 1.1/3.3 的自动化测试作为本变更的回归证据。验证：`node scripts/verify/manifest.json` 相关场景命令退出码符合预期（绿色通过）。
  - 结论：既有检查项 `scripts/verify/checks/task-9.1.mjs`（manifest `DAS-R06-S03`「首实例尚未就绪」的执行命令）已存在；本变更增强其注入证明——第 5 项断言 frontend-ready 门：`drain_pending_opens` 必须经 `mark_frontend_ready`（`.setup()` 末尾不得再 drain，WebView 未加载会丢）、命令已注册、`App.tsx` 在订阅 effect 后发就绪信号，均为此前缺失的可证伪防线。另顺带修复了被 task-9.1 clippy 第 1 步触发的预存全仓 clippy 红线（与 file-open 无关，见 4.2 说明）。验证：`node scripts/verify/checks/task-9.1.mjs` 退出 0。
- [x] 4.2 全量验证：`cargo test --workspace`、`pnpm test`、`pnpm typecheck`、`pnpm lint`、`pnpm format:check` 全部通过；若 CI 有 macOS 构建/打包门禁，核对 `apps/desktop` 构建成功。验证：上述命令全绿。
  - 顺带修复（为达成全绿所做的预存清理，均与 file-open 逻辑无关）：`clippy --workspace --all-targets --all-features -- -D warnings` 在 HEAD 上原本有 7 处预存错误（`services/mod.rs`/`services/library.rs` 缺 `#[must_use]`、`snapshot.rs` f64→u64 cast、函数过长），已修复成绿；`ImmersivePlayer.tsx` 一处 prettier 格式（未修改过文件）已格式化。`check-embedded-frontend.mjs` 用于核对该二进制嵌入的 dist 是最新产物，`cargo build -p echo-app` 后验证通过。

- [x] 4.3 人工验收（本机 macOS）：`确定 Echo 完全退出后`，在访达双击一个音乐文件 → Echo 启动且该文件开始播放；再次双击另一文件（Echo 运行中）→ 立即切换播放；重复双击同一文件 → 播放一次不重复处理。验证：三场景均符合预期，无"只开软件不播放"现象。
  - 取证方式：临时探针（`fn probe`，打完即撤）+ `CGWindowListCopyWindowInfo` 取窗口 id + `screencapture -l` 窗口级截图（见 design D7）。
  - 冷启动双击 `probe-tone.mp3`：探针顺序 `boot → RunEvent::Opened → deliver_file_open → setup: supervisor managed → … → drain_pending_opens drained=["/private/tmp/probe-tone.mp3"] → emit_file_opens paths=[…]`；`desktop-state.json` 的 `playbackSession.current` 与 history `played_at_ms` 同步更新为该临时项。**修复前同一实验输出的是 `deliver_file_open … -> NO supervisor (dropped)` 且 `drained=[]`。**
  - 运行中双击第二文件：探针只剩 `RunEvent::Opened → deliver_file_open → emit_file_opens`（无 drain，即走立即投递），`current` 随之切换。**通过。**
  - "真在播"用 30s 长音频复核：间隔 3s 两帧窗口截图，播放进度点位移 ≈120px ≈ 3.4s（与真实时间一致），传输键为暂停态。**通过。**

## 5. 根因补齐：supervisor 创建早于 `Builder::build()`（D6，真机取证后才定位）

- [x] 5.1 `main()` 顶部（`tauri::Builder::default()` 之前）创建进程级唯一 `Arc<StartupSupervisor>`；`deliver_file_open` 改为接收 `startup: &StartupSupervisor`（不再 `try_state`），`RunEvent::Opened` 与单实例 argv 两条路径均捕获该 `Arc` 传入。验证：`cargo check -p echo-app` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- [x] 5.2 `.setup()` 仍 `app.manage(Arc::clone(&startup))`，`file_open_frontend_ready` 命令改用 `State<'_, Arc<StartupSupervisor>>` 取同一实例（落实 D3 单一事实；`wire_composition(app, &startup)` 也接收该实例）。验证：`cargo test --workspace` 全绿（0 失败）。
- [x] 5.3 `scripts/verify/checks/task-9.1.mjs` 增加第 6 项断言并做变异证明：① `Arc::new(StartupSupervisor::new())` 下标必须小于 `tauri::Builder::default()`；② 全仓不得再出现 `try_state::<Arc<StartupSupervisor>>`；③ 至少两条 open 路径以 `deliver_file_open(app, &startup` 形式传入捕获实例。验证：门禁绿；三条变异各自失败并给出对应消息（A `try_state` 回退 / B 创建晚于 Builder / C 仅 1 处传捕获实例 → 全部 exit 1），恢复后复跑 exit 0。
