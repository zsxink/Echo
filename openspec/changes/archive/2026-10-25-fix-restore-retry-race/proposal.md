# fix-restore-retry-race

## Why

main 的 CI 在合并 PR #95 后失败(issue #96):`explicit_restore_retry_clears_exhaustion_and_restarts_the_bounded_worker` 超时。根因是 `SyncRuntime::retry_restore()` 只清状态标志，既不推进 generation 也不回收旧 worker——而 `restore_retry.rs` 模块文档明确要求显式重试要推进 generation。旧 worker 在「耗尽标志已置位、线程尚未退出」的窗口内会让紧随其后的 `start_restore_retry_with_delays()` 命中幂等分支静默返回，新一轮尝试从未开始。产品路径 `sync_restore_retry` IPC 命令同样受影响：用户的重试点击可能被静默吞掉。

## What Changes

- `SyncRuntime::retry_restore()` 复用 `cancel_restore_retry()`(推进 generation + `wake.cancel()` + join 旧线程 + 清尝试计账)，再设置 Disabled/清错误/推进事件序号；返回后随后的 `start_restore_retry()` 必然开启全新的有界尝试序列。
- 行为语义与 `stop()`/`pause()` 等既有用户转换对齐(它们本来就持 `transition` 锁调用同一交接函数,无新死锁面)。
- 不改任何对外 API 签名、IPC 协议或持久化格式。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `lan-library-sync`:「显式启用与可信设备加入」需求补充显式重试的语义——用户显式重试 MUST 使旧的自动恢复 worker 立即失效并被回收，后续重启 MUST 开启全新有界尝试序列，不得被旧 worker 的收尾窗口静默吞掉。

## Impact

- `apps/desktop/src-tauri/src/sync_runtime.rs`:`retry_restore()` 一个函数体(~10 行)。
- 现有测试 `explicit_restore_retry_clears_exhaustion_and_restarts_the_bounded_worker` 即回归证明(CI 已证明其能捕获该竞态)。
- 关联 issue:#96;引入该机制的变更归档于 `2026-10-23-persist-lan-sync-session-intent`。
