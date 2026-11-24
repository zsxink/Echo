## Why

Issue #174: 默认重型场景耗时约 5:37，却没有以 1,000 / 10,000 首与 50 ms 门槛执行 Release 计时。将正确性和计时分开，使日常 CI 更短而保留证据。

## What Changes

保留 workspace 的小样本导入可见性、保存确认、歌单持久化测试，新增定时及手动的 Release 计时，默认报告明确递延计时。保留既有 50k、覆盖率和审计策略。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`：明确日常正确性、独立性能入口及报告。
- `library-experience`、`playlist-management`：将正确性与 Release 计时分成独立场景，不改变当前业务要求。

## Impact

影响 CI、场景登记/报告、Rust 测试和必要的阶段计时诊断；不改变业务语义。
