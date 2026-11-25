## Context
Task 1.7 先执行 cargo-deny 许可、依赖禁用和来源检查，再在 main / 定时模式执行真实公告检查。现有 Cargo 图对这两个 crate 使用兼容补丁约束。

## Goals / Non-Goals
只变更两个公告直接涉及的锁版本及必要传递依赖，对齐 `LICENSES.md`；不改变 Core、Desktop、Tauri 或同步协议接口，不迁移 major 版本，不改审计策略。

## Decisions
- 使用 `cargo update -p <crate> --precise <version>` 解析；`url 2.5.9` 要求 `form_urlencoded ^1.2.3`，允许这一个必要联动，不刷新全图。
- 检查基线和候选均使用相同审计库和忽略配置；前者应报告这两条公告，后者不应报告这两条。如果其他告警仍失败，保留真实结果，不误报整个审计通过。
- 用 crates.io 元数据及仓库入口核对包版本和许可；若在线源无法访问，明确标注哪些检查只能使用缓存数据库、哪些未执行。

## Risks / Trade-offs
最窄版本迁移仍会重编依赖层；新增的 URL 来源解析行为由上游兼容修复定义。仅通过离线模式不能证明 RustSec 公告已经清除。
