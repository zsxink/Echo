## Context

见 `proposal.md` 的 Why。设计上需要解决的不是功能实现，而是三份事实来源的**同步时序**：

- `openspec/specs/**/spec.md` 是已发布规格，也是场景 ID 的唯一派生源（`scripts/verify/spec-scenarios.mjs` 按 heading 顺序生成 `<AREA>-R<NN>-S<NN>`，**不扫描 change delta**）。
- `docs/traceability.md` 与 `scripts/verify/manifest.json` 必须与主规格的场景 ID 集合完全相等（`scripts/verify/reconcile-scenarios.mjs`）。
- 当前分支上主规格已被 `41692a8` 写入 15 个未登记场景，导致 `spec 438 / trace 423 / manifest 423`。

约束：`AGENT.md` 要求 change 在提 PR 前归档，归档动作会把 delta 原样合并回 `openspec/specs/`。因此本 change 的「回退主规格」必须是**暂时**的，归档时会重新加上同一批文本。

## Goals / Non-Goals

**Goals:**

- 让 `openspec/specs/` 在当前分支恢复到可核对的基线（场景 ID 集合 = `traceability.md` = `manifest.json`），使 `scenario reconciliation` 对**任何 change**（包括本分支的音效 change）都不再因这批文本变红。
- 把这批文本的所有权从「已发布规格」转移到 change，使后续实现、测试、场景登记都在同一 change 下完成。
- 保留原文，不重新表述要求文本，避免在搬运过程中产生规格语义漂移。

**Non-Goals:**

- 不决定这批要求的实现顺序，也不拆分 change。
- 不修改场景 ID 派生规则或 reconciliation 脚本——门禁语义本身没有问题，问题在于主规格被提前写入。
- 不在本 change 阶段登记场景（见 Decisions 第 3 条）。

## Decisions

### 1. 搬运 delta，而不是登记场景

两个可选路径：把 15 个场景登记进 `traceability.md` + `manifest.json`（`spec 438 = trace 438`），或把文本搬出主规格（`423 = 423 = 423`）。

选后者，因为登记要求每个场景有**可执行的 command** 和对应的 `tests/scenarios/<ID>.yaml`；其中 `IL-R11-S03`、`PM-R13-S01..S04` 共 5 个场景在 `apps/desktop/src` 尚无实现（`grep "全部歌单"` 零命中），13 个场景无隔离测试。登记它们等于把未实现的断言写成门禁的通过项，违反 `AGENT.md` 的「未归档的 change 不得提 PR / 主规格承载已验证行为」。搬运则不需要任何测试先落地。

代价：归档前必须再实现并登记，否则本 change 会长期停留在 `openspec/changes/`。

### 2. 用 `## ADDED Requirements` 而不是 `## MODIFIED`

`immersive-lyrics` 和 `playlist-management` 都是既有 capability，本次是在其下**新增**两个 Requirement，不改动既有 Requirement 的文本，因此 delta 用 `ADDED`。按 `openspec instructions specs` 的规则，既有 capability 的 delta 不写 `## Purpose`。

### 3. 本 change 的阶段划分

实现完成前的阶段**只做规格归位**：delta 文件 + `openspec/specs/` 回退同一次提交落地。场景登记（`scenario-commands.mjs` → `gen-scenario-manifests.mjs --write` → `traceability.md`）留到实现与验证完成之后、归档之前，因为它与实现状态强耦合。

派生的时序要求：**回退主规格与 delta 落地必须原子**。若只回退不落 delta，这批要求会短暂失去唯一载体；若只落 delta 不回退，reconciliation 仍红。

### 4. 归档时的净变化为零

归档把 delta 合并回主规格，`openspec/specs/` 回到与当前分支相同的文本（场景数仍为 438），届时应同时登记 15 个场景，使三方在 438 上重新一致。也就是说本 change 对主规格的**最终**影响是「新增 15 个已登记场景」，而非「回退」。

## Risks / Trade-offs

- [归档前若有人直接在主规格重新写入这批文本，会与本 change 的 delta 冲突] → 在 `proposal.md` 与本文件中记录所有权；delta 文本与 `41692a8` 的原文逐字一致，冲突可机械判定。
- [回退主规格使这 15 个要求短暂不在「已发布规格」内，可能被误认为需求撤回] → 文本在 change delta 中原样存在，`docs/traceability.md` 不变；change 的 `proposal.md` 明确记录其现状与后续实施路径。
- [15 个场景里 13 个无隔离测试，实现阶段可能发现要求本身需要调整] → 那属于实现阶段的 delta 修订，本阶段不做前瞻性修改。
- [`IL-R11` 与 `PM-R13` 都无法部分登记（reconciliation 按场景集合等值比对）] → 实现阶段按 Requirement 整体推进并整体登记，不按场景零散登记。

## Migration Plan

1. 本提交：写入 change 的 `proposal.md` / `specs/` / `design.md` / `tasks.md`，并把 `openspec/specs/immersive-lyrics/spec.md`、`openspec/specs/playlist-management/spec.md` 回退到 `main`。
2. 验证：`node scripts/verify/reconcile-scenarios.mjs` 回到 423 一致；`pnpm exec openspec validate align-immersive-player-and-playlist-directory --strict` 通过。
3. 实现阶段（后续 change 之外的独立工作）：实现 `PM-R13`、`IL-R11-S03`，补隔离测试，登记 15 个场景。
4. 归档：`openspec archive` 合并 delta，主规格恢复 438 场景且三方一致。

回滚：整批改动只涉及两个主规格文件的回退和 change 目录的新增文件，`git revert` 单个提交即可恢复 `41692a8` 之后的状态。
