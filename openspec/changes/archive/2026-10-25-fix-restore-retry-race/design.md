# fix-restore-retry-race — 设计

## Context

竞态链路(代码推演 + CI 日志证实):worker 线程在 `mark_restore_exhausted()` 置位标志后、线程退出前存在窗口;`retry_restore()` 只清标志;紧随的 `start_restore_retry_with_delays()` 看到 `!worker.thread.is_finished()` 即静默 `return Ok(())`(为启动期重复自动恢复设计的幂等分支),新 worker 不再启动。`restore_retry.rs` 模块文档要求显式重试推进 generation,`stop()`/`pause()`/`leave()`/root 切换均通过 `cancel_restore_retry()` 完成交接,唯独重试路径遗漏。

## Goals / Non-Goals

**Goals:** 显式重试后必然开启全新有界尝试序列;旧 worker 无论处于退避等待还是收尾窗口都立即失效并被回收;现有 CI 测试稳定通过。

**Non-Goals:** 不改自动恢复的幂等启动语义(启动期重复 `start_restore_retry` 仍应跳过);不改尝试次数/退避常量;不改 IPC 或持久化。

## Decisions

**D1:`retry_restore()` 内联复用 `cancel_restore_retry()` 而非只修 `is_finished` 判定。** 备选是把 `start_restore_retry_with_delays` 的幂等分支改成「等待/回收旧线程再启动」——但那会改变自动启动路径语义并引入启动期 join 阻塞;而 `retry_restore` 是显式用户动作,本就该与 stop/pause 同级交接。死锁安全:`stop()`/`pause()` 已在持有 `transition` 锁时调用 `cancel_restore_retry()`(join 旧线程),证明 worker 路径不取 `transition` 锁,`retry_restore` 同样持该锁调用无新增死锁面。join 的等待上界 = 旧线程当前动作结束(退避中被 `wake.cancel()` 立即唤醒;尝试中则等待该次尝试返回,与 stop 语义一致)。

**D2:状态复位保持原序。** `cancel_restore_retry()` 已清 `restore_attempts`/`restore_exhausted`;Disabled/`last_error=None`/`event_sequence += 1` 仍由 `retry_restore` 在其后设置,对外可见行为不变。

## Risks / Trade-offs

- [`retry_restore` 变为可能等待旧线程收尾(极端情况一次在途尝试)] → 与 `stop()` 完全相同的既有语义,IPC 命令本就运行在 `spawn_blocking` 线程,不阻塞 UI。
- [自动启动路径未改动,理论上仍可有重复 start 被幂等跳过] → 这是设计意图(启动幂等),非本缺陷。

## Migration Plan

单函数体替换,无数据迁移;revert 即回滚。回归证明 = 现有测试(它在 CI 负载下已经捕获过该竞态),另以循环运行加固置信。

## Open Questions

（无）
