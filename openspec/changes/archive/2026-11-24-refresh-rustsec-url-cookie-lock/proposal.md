## Why
main 的依赖审计报告 `url 2.5.8`（RustSec RUSTSEC-2026-0592）和 `cookie 0.18.2`（RUSTSEC-2026-0594）。保留原审计阈值，修复已发布兼容补丁的锁定版本。

## What Changes
- 精准升级 `url` 到 2.5.9、`cookie` 到 0.18.3，以及 `url` 新约束所需的 `form_urlencoded` 1.2.3；将许可清单与实际解析结果一致。
- 以 Task 1.7 现有完整入口验收；若有额外审计问题，区分真实发现与环境故障，不以忽略或放宽政策换取通过。

## Capabilities
### New Capabilities
无。

### Modified Capabilities
无。此处维持 `engineering-governance` 已有依赖审计要求，仅维护锁定的依赖及许可清单。

## Impact
限 Rust 工作区依赖解析和许可清单。产品、架构、路线图及界面契约不变；不调整 CI、审计脚本、忽略列表或前端依赖。Windows 媒体目录符号链接测试独立处理。
