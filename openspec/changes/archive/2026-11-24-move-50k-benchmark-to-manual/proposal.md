## Why

将每次 PR / main 默认 CI 上的重型 50,000 首合成曲库 SQLite 基准及产物上传改成按需，减少日常成本，同时保留真实测量入口和现有 PRD 预算。

## What Changes

- 默认 `ci.yml` 不执行 task 12.5，也不上传 benchmark-50k。新增仅 `workflow_dispatch` 的同名按需基准入口，同时保留本地 `pnpm verify:task -- 12.5`。
- 默认 release gate 报告 LE-R07-S01 未运行、需手动测量，不算通过或已被其他默认 job 覆盖；直跑场景及本地完整验证仍执行真实命令。
- 不改 50k 测量预算与输出 JSON，不改 `pnpm verify:perf` 和其现有路径筛选、前端虚拟列表测试、任何平台门禁或依赖审计。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `engineering-governance`：显式的手动性能测量与默认 CI 结构证据。

## Impact

仅影响工程治理层 GitHub Actions、场景检查、测试和规格，不触及产品功能、运行时架构或数据协议。对应 GitHub Issue [#171](https://github.com/zsxink/Echo/issues/171)。
