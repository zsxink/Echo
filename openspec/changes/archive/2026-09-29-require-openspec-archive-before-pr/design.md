## Context

仓库已要求功能变更通过 Issue、独立分支和 Pull Request 进入 `main`，并要求 OpenSpec change 在完成后同步、归档；但没有明确规定归档必须早于创建 Pull Request，导致已合并的实现仍可能留下活动 change 和未同步 delta。

## Goals / Non-Goals

**Goals:** 在所有维护入口统一说明：实现和规定验证完成后，先同步并验证 delta spec，再归档 change，最后创建 Pull Request；无 delta 的 change 仍需归档。

**Non-Goals:** 不增加自动化门禁、不改变 GitHub 分支保护、不修改运行时代码，也不重新打开或修改已合并的产品 PR。

## Decisions

- 在 `AGENT.md` 作为执行顺序约束，在 `openspec/config.yaml` 作为 archive 操作指引，在 `openspec/CODE_STANDARDS.md` 作为交付规范，在 GitHub Pull Request 模板中加入归档状态核对项。
- 在 `engineering-governance` capability 中定义可验证要求，确保规范和维护入口一致。
- OpenSpec delta 有变更时先同步到主规格并验证；没有 delta 时直接完成归档。任何未归档的 change 都不能进入 Pull Request 创建阶段。

## Risks / Trade-offs

- 手工流程仍依赖维护者遵守。Pull Request 模板提供提交时的可见核对项，后续若要自动阻断，可另行引入 CI 门禁。

## Migration Plan

更新仓库指引、操作规则、交付规范和模板；同步本 change 的新增 requirement 并归档。无需迁移既有数据或调整代码。
