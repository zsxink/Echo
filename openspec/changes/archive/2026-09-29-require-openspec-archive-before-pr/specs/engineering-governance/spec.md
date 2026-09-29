## ADDED Requirements

### Requirement: OpenSpec change 必须在创建 Pull Request 前同步并归档
每项通过 OpenSpec 管理的变更 MUST 在创建 Pull Request 前完成实现任务和规定验证。存在 delta spec 时 MUST 先将其同步到对应主规格并验证同步结果，然后归档 change；没有 delta spec 时 MUST 在创建 Pull Request 前归档 change。未归档的 change MUST NOT 进入 Pull Request 创建阶段。

#### Scenario: 存在 delta spec 的变更准备提交 Pull Request
- **WHEN** OpenSpec change 的实现任务和规定验证均已完成且存在 delta spec
- **THEN** 维护者先同步 delta 到主规格并验证，再归档 change，之后才创建 Pull Request

#### Scenario: 没有 delta spec 的变更准备提交 Pull Request
- **WHEN** OpenSpec change 的实现任务和规定验证均已完成且没有 delta spec
- **THEN** 维护者先归档 change，之后才创建 Pull Request

#### Scenario: change 尚未归档
- **WHEN** 维护者准备创建 Pull Request 但对应 OpenSpec change 仍在活动目录
- **THEN** 交付流程要求先完成必要的规格同步和归档，Pull Request 模板明确提示该状态
