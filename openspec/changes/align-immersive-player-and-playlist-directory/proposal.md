# Proposal

## Why

commit `41692a8`（「新增音效与均衡器 change」）把工作区里两批与本分支主题无关的规格改动一并提交了：沉浸式播放器的布局/主题要求，以及歌单目录与封面编辑要求。这两批文本**直接写进了 `openspec/specs/`**，既没有对应的 OpenSpec change，也没有在 `docs/traceability.md` 和 `scripts/verify/manifest.json` 登记场景。

后果是发布门禁的三方集合对不上：`spec 438 = trace 423 = manifest 423`，`pnpm verify:governance` 在第一项 `scenario reconciliation` 就红，且该失败与本分支的音效工作无关、无法在音效 change 内消化。缺登记的 15 个场景为 `IL-R10-S01..S05`、`IL-R11-S01..S03`、`PM-R13-S01..S04`、`PM-R14-S01..S03`。

把未完成的工作放进 `openspec/specs/`（「已发布的规格」）本身就是流程倒置：主规格只应承载已经实现并验证的行为，未实现的行为属于 change。本 change 收拢这批工作，让主规格回到可核对的基线。

## What Changes

- 把 `immersive-lyrics` 的两个新增 Requirement（沉浸式布局与歌词定位的几何对齐、沉浸模式的主题连续与分层退出）和 `playlist-management` 的两个新增 Requirement（全部歌单目录与编辑一致、封面预览与可恢复状态）从 `openspec/specs/` 移出，**文本原样**作为本 change 的 delta spec。
- `openspec/specs/immersive-lyrics/spec.md` 与 `openspec/specs/playlist-management/spec.md` 回退到 `main` 的版本，`scenario reconciliation` 恢复 `spec 423 = trace 423 = manifest 423`。
- 同一次提交里另有一行非 Requirement 的增量，一并移出主规格并在此留存原文，避免静默丢失：

  > 界面基准：`docs/prototype/echo-desktop-player.html`（2026-09-30 对照）。原型中的测试歌词、固定曲目与模拟反馈不作为播放或持久化成功的依据；歌词来源、seek 与手动滚动规则仍按本规格执行。

  该行不产生场景 ID，不影响 reconciliation；归档时应与 IL 要求一同回到主规格。
- 记录这批要求的当前落地状态，作为后续实施与验收的起点：
  - `IL-R10`（宽屏/中等宽度/窄屏布局、歌词按实际高度居中、首尾留白）已在 `main` 实现（`apps/desktop/src/styles/player.css`、`styles/responsive.css`、`features/player/LyricsColumn.tsx`、`useLyricsStartOffset.ts`），但只有宽泛的浏览器 e2e（`task-13.1.mjs` 覆盖的 A9）间接触及，没有隔离断言，无 vitest 覆盖。
  - `IL-R11-S01`（主题同步）已有 `--player-background` 变量写入，但三个表面同步未被断言；`IL-R11-S02`（分层收起）在 `ImmersivePlayer.test.tsx` 已有通过用例；`IL-R11-S03`（滚动后 180ms 误触防护）**尚未实现**。
  - `PM-R13`（「全部歌单」目录、排序、拖动、编辑入口）**尚未实现**；`PM-R14` 的封面选择/预览/失败处理已在 `PlaylistNameDialog.tsx` 实现，但无前端测试（仅 `crates/echo-desktop` 有后端封面持久化用例）。
- 明确非目标：本 change 只做规格归位与现状登记，**不实现**歌单目录、拖动排序、180ms 误触防护，**不写**测试，**不登记**场景。场景登记与 delta 同步发生在实现完成、验证通过、归档时（见 `AGENT.md` 的 OpenSpec 流程）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `immersive-lyrics`: 新增要求「沉浸式布局与歌词定位必须以实际几何尺寸对齐」「沉浸模式必须使用连续的主题背景与分层退出」。
- `playlist-management`: 新增要求「全部歌单目录与歌单编辑必须保持一致」「编辑歌单封面必须提供预览与可恢复状态」。

## Impact

- 只影响 `openspec/specs/` 中两个规格文件、本 change 目录，以及后续实现该功能时的桌面前端代码；`docs/traceability.md` 与 `scripts/verify/manifest.json` 在本阶段**保持 423 条不变**（这批场景尚未实现，不应提前登记为可执行场景）。
- 不修改生产代码、不改 CI 配置、不改门禁脚本。
- 实施阶段涉及 `apps/desktop/src/features/player/`（布局几何、主题同步、误触防护）、`apps/desktop/src/features/playlists/`（目录、排序、拖动、封面）与 `apps/desktop/src/app/`（导航入口、视图类型），遵循 `docs/DESIGN.md` 分层与 `openspec/CODE_STANDARDS.md`；界面基准为 `docs/prototype/echo-desktop-player.html` 与 `docs/interface-terminology.md`（原型中的模拟存储不作为实现依据）。
- 本 change 处于规划状态：`openspec/specs/` 的回退与 delta 落地同时发生，归档时 delta 内容原样回到主规格，净变化为主规格不变、场景登记补齐。
