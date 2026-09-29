## Why

PR #43 合并后发现对应 OpenSpec change 仍处于活动目录，且 delta spec 未进入主规格。仓库需要在提交 Pull Request 前设置明确的规格同步与归档门槛，避免实现、主规格和归档记录脱节。

## What Changes

- 规定 OpenSpec change 的实现任务和验证完成后，先同步 delta spec 并归档，再创建 Pull Request。
- 将门槛同步到仓库 Agent 指引、OpenSpec 操作规则、工程交付规范和 Pull Request 模板。

## Capabilities

### Modified Capabilities

- `engineering-governance`: 为变更交付流程增加 OpenSpec 同步与归档前置条件。

## Impact

只影响维护者的变更交付流程和相关文档，不改变产品行为、运行时代码、CI 命令或分支保护设置。
