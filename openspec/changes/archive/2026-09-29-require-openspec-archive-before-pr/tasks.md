## 1. 流程规范

- [x] 1.1 在 Agent 指引、OpenSpec archive 操作规则、代码交付规范和 Pull Request 模板中明确“先同步 delta spec、再归档、后创建 PR”。
- [x] 1.2 更新 `engineering-governance` capability，定义有 delta、无 delta 和尚未归档时的交付行为。

## 2. 验证

- [x] 2.1 运行 `openspec validate --specs` 并确认主规格有效。
- [x] 2.2 运行 `git diff --check` 确认改动无空白错误，并检查流程说明一致。
