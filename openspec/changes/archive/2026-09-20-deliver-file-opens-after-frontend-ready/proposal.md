## Why

Echo 未运行时，在访达双击音乐文件，Echo 只启动、不播放该文件；Echo 运行中双击则正常播放。根因是 macOS 冷启动的 `odoc` 文件打开事件到达时，前端 WebView 的 JS 事件监听器尚未注册，`app.emit("app://file-open-request", …)` 因没有 JS 监听器而被 Tauri 静默丢弃（`tauri/src/event/listener.rs::emit_js_filter` 仅注入已注册监听器，无缓存、无重试）。这违反了已批准规格 `desktop-app-shell`「首实例尚未就绪：壳暂存请求直到播放器可用或明确报告失败，…不静默丢弃路径」——现有暂存只覆盖 supervisor `ready` 之前，未覆盖"前端监听器就绪"之前。

## What Changes

- 将"前端 WebView 已完成 file-open 事件监听器注册"确立为 file-open 投递就绪的第二个门（在 supervisor 的 startup 就绪之外）。
- 在壳层为 file-open 请求增设一个前端就绪感知的投递队列：后端在收到 OS 文件打开请求时，若前端监听器尚未就绪则暂存；前端注册完成后显式拉起排空并补发，不再静默丢弃。
- `StartupSupervisor` 的 pre-ready FIFO 语义保持不变（仍覆盖"supervisor 尚未 ready"的窗口），新增逻辑与它衔接，二者共同保证任意时刻到达的 file-open 都不丢。
- 前端在 `useEffect` 注册 `app://file-open-request` 监听器之后，调用新增的排空命令触发补发（对运行中实例为无操作/幂等）。
- 为冷启动 file-open 补一条自动化回归断言，证明"前端监听器注册前到达的路径最终被播放"。

## Capabilities

### New Capabilities

- _无。_

### Modified Capabilities

- `desktop-app-shell`: 修改「应用必须保证单实例与文件打开唤醒」下「首实例尚未就绪」场景的契约边界——把"播放器可用"细化为"前端 file-open 事件监听器已就绪"，并明确补发保证：在前端就绪前到达的路径不得静默丢弃，必须在就绪后按到达顺序补发。这是在既有能力上的行为约束细化，而非新能力。

## Impact

- **壳层** `apps/desktop/src-tauri/src/main.rs`：`deliver_file_open` / `drain_pending_opens` 与新的前端就绪门结合；新增一个 IPC 命令用于前端注册后拉起排空。
- **运行时** `crates/echo-desktop/src/runtime/mod.rs`：`StartupSupervisor` 增加"前端就绪"状态与对应的暂存/排空语义。
- **前端** `apps/desktop/src/app/App.tsx`：`useEffect` 在 `listen` 成功后调用排空命令。
- **事件契约** `apps/desktop/src-tauri/src/main.rs::FILE_OPEN_REQUEST` 行为不变（仍是单一 path 数组事件），新增的是"何时补发"规则。
- **测试**：`App.test.tsx` 现有 emit 后断言保持不变可作基线；新增后端单元测试覆盖"前端未就绪→暂存→就绪后排空补发"；`native_e2e.rs` / 门禁场景如适用则更新。

无新增第三方依赖。改动集中在壳层与运行时的 file-open 投递路径，Core 领域逻辑不受影响。