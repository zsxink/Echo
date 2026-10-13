## 1. 规格归位（本阶段）

- [ ] 1.1 确认 `openspec/specs/immersive-lyrics/spec.md` 与 `openspec/specs/playlist-management/spec.md` 中新增的四个 Requirement 已逐字复制到本 change 的 `specs/` delta；用 `git show origin/main:openspec/specs/immersive-lyrics/spec.md` 等命令比对，确认主规格文本与本 change delta 之外无其他差异
- [ ] 1.2 把两个主规格文件回退到 `main`（`git checkout origin/main -- openspec/specs/immersive-lyrics/spec.md openspec/specs/playlist-management/spec.md`），与 1.1 的 delta 落地处于同一提交
- [ ] 1.3 运行 `node scripts/verify/reconcile-scenarios.mjs`，确认输出回到 `spec 423 = trace 423 = manifest 423`、无 mismatches
- [ ] 1.4 运行 `pnpm exec openspec validate align-immersive-player-and-playlist-directory --strict` 与 `pnpm exec openspec validate --archived`，确认两项均通过

## 2. 沉浸式布局几何对齐（IL-R10）

- [ ] 2.1 核对宽屏双栏（>980px）下歌曲信息顶部对齐全唱臂支点、歌词从信息下方开始的既有实现（`apps/desktop/src/styles/player.css`、`features/player/LyricsColumn.tsx`），补隔离断言覆盖 `IL-R10-S01`
- [ ] 2.2 实现/核对按换行后实际高度居中的定位（`useLyricsStartOffset.ts` 或等价逻辑），并补断言覆盖 `IL-R10-S02`（多行长句）
- [ ] 2.3 核对首尾留白使首句与末句仍可到达对齐目标，补断言覆盖 `IL-R10-S03`
- [ ] 2.4 核对 761–980px 居中纵向重排与 ≤760px 五行预览独立滚动（`styles/responsive.css`），补断言覆盖 `IL-R10-S04`、`IL-R10-S05`
- [ ] 2.5 运行 `pnpm --filter desktop test -- --run` 与相关浏览器 e2e，确认 2.1–2.4 新增断言全部通过

## 3. 主题连续与分层退出（IL-R11）

- [ ] 3.1 核对沉浸式播放器、常驻播放栏、播放队列三处背景均由同一主题派生（`--player-background` 变量链），补断言覆盖 `IL-R11-S01`
- [ ] 3.2 确认 `ImmersivePlayer.test.tsx` 中已有的分层收起用例对应 `IL-R11-S02`，必要时补齐 Escape 无更高层浮层的分支
- [ ] 3.3 实现滚动结束后 180ms 内的误触防护，使区域点击不切换歌词专注状态且不破坏既有手动滚动规则，补断言覆盖 `IL-R11-S03`
- [ ] 3.4 运行 `pnpm --filter desktop test -- --run`，确认 IL-R11 三条场景断言通过

## 4. 全部歌单目录（PM-R13）

- [ ] 4.1 在 `apps/desktop/src/features/playlists/` 与 `apps/desktop/src/app/` 增加「全部歌单」入口与视图类型，使激活后显示歌单卡片与实际总数、不展示歌曲选择列或批量工具，覆盖 `PM-R13-S01`
- [ ] 4.2 实现名称/创建时间 × 升序/降序排序（默认创建时间降序），并让侧边歌单导航与目录保持同序，覆盖 `PM-R13-S02`
- [ ] 4.3 实现拖动歌单卡片调整条目顺序（仅改展示顺序，不改成员与内部歌曲顺序），覆盖 `PM-R13-S03`
- [ ] 4.4 打通卡片编辑按钮、详情编辑按钮、上下文菜单三个入口到同一编辑对话框，且不触发播放或卡片导航、关闭后恢复焦点，覆盖 `PM-R13-S04`
- [ ] 4.5 运行桌面前端测试，确认 PM-R13 四条场景断言通过

## 5. 封面预览与可恢复状态（PM-R14）

- [ ] 5.1 核对 `PlaylistNameDialog.tsx` 已有的封面预览与选择入口，补前端断言覆盖 `PM-R14-S01`（预览并保存）与 `PM-R14-S02`（取消不修改）
- [ ] 5.2 核对图片读取/保存失败路径的提示与状态保留（名称、已有封面、可重试入口），补断言覆盖 `PM-R14-S03`
- [ ] 5.3 确认保存成功后目录卡片与侧边导航封面同步、重命名不丢封面关联，补断言或复用后端用例
- [ ] 5.4 运行桌面前端测试，确认 PM-R14 三条场景断言通过

## 6. 场景登记与归档

- [ ] 6.1 在 `scripts/verify/scenario-commands.mjs` 为 15 个场景填入已实现且可执行的 command，运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 重新生成 `scripts/verify/manifest.json` 与 `tests/scenarios/*.yaml`（不得手工编辑 manifest）
- [ ] 6.2 同步 `docs/traceability.md`，运行 `node scripts/verify/reconcile-scenarios.mjs` 确认 `spec 438 = trace 438 = manifest 438`、无 mismatches
- [ ] 6.3 运行 `node scripts/verify/validate-scenario-commands.mjs`、`node scripts/verify/check-scenario-churn.mjs`，确认命令有效且未突破冻结上限
- [ ] 6.4 运行 `pnpm verify:governance` 完整门禁，确认全部检查通过
- [ ] 6.5 归档前把 `proposal.md` 里留存的「界面基准」原文行加回 `openspec/specs/immersive-lyrics/spec.md` 的 Purpose 之下
- [ ] 6.6 归档本 change（`openspec archive`），确认主规格回到 438 场景且三方一致、`pnpm exec openspec validate --archived` 通过
