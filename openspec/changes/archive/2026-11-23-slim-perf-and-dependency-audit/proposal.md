## Why

跟踪：[#165](https://github.com/zsxink/Echo/issues/165)。重型 release 性能场景在与之无关的 PR 上也会占用 Linux runner；依赖审计的在线分支重复执行了已经完成的本地政策检查。保留质量信号的前提下，减少确定没有新增信号的工作。

## What Changes

- 保留 task gate 和 50k benchmark，对 PR 内的一小组已审核路径单独跳过重型性能场景；无法确定时继续执行。
- 将 Cargo 的 license、bans、sources 合成一次政策检查，在线只追加一次 Cargo advisory 检查，保留其余审计及失败处理。
- 同步治理规格、workflow 结构门禁与回归验证。

## Capabilities

### Modified Capabilities

- `engineering-governance`：CI 执行分层与依赖审计检查分工。

## Impact

仅影响 CI workflow 与验证脚本；不改变产品、原生验证、任务分片、benchmark 或发布门禁。产品、架构、路线图和界面基准都不变；脚本遵守 `openspec/CODE_STANDARDS.md` 的单一职责、验证和无新增依赖约束。
