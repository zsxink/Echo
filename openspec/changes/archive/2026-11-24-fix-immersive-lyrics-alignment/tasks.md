## 1. 稳定几何信号

- [x] 1.1 在 `apps/desktop/src/features/player/useImmersiveTransition.ts` 中派生 `settled`：`opening` / `closing` / `closed` 为 `false`，`open` 在 `IMMERSIVE_OPEN_MS` 后为 `true`，减少动画下进入 `open` 即为 `true`；清理计时器并覆盖 `closing → open` 中断恢复。在 `useImmersiveTransition.test.tsx` 补测试并用 `pnpm --filter @echo/desktop test -- --run src/features/player/useImmersiveTransition.test.tsx` 验证
- [x] 1.2 核对 `IMMERSIVE_OPEN_MS` 与 `apps/desktop/src/styles/tokens.css` 的 `--motion-base` 一致，并在 `immersiveTransitionStyles.test.ts` 增加静态断言；运行 `pnpm --filter @echo/desktop test -- --run src/features/player/immersiveTransitionStyles.test.ts`

## 2. 歌词列接入与组件回归

- [x] 2.1 给 `apps/desktop/src/features/player/LyricsColumn.tsx` 增加 `geometrySettled: boolean` 输入，并把它加入对齐 effect 依赖；为 `false` 时直接返回，不创建 `ResizeObserver`、不排 rAF、不写 `scrollTop` / `--lyrics-edge-*`。由 `ImmersivePlayer.tsx` 的 `ImmersiveBody` → `SmoothLyricsColumn` 透传 `settled`；运行 `pnpm --filter @echo/desktop typecheck` 与 `pnpm --filter @echo/desktop test -- --run src/features/player/ImmersivePlayer.test.tsx` 确认既有行为不回归
- [x] 2.2 新建 `apps/desktop/src/features/player/LyricsColumn.test.tsx`，用矩形与 `matchMedia` stub 覆盖：入场未稳定时不写滚动；稳定后当前行中心对齐唱片中心（宽屏）与歌词视口中心（窄屏/专注）；暂停展开同样对齐；歌词晚到后重新对齐；切歌后重新对齐；减少动画下立即对齐。运行 `pnpm --filter @echo/desktop test -- --run src/features/player/LyricsColumn.test.tsx`
- [x] 2.3 复核 `closing → open`、退场中切歌与退出歌词专注阅读路径：确认 `settled` 在重新进入 `open` 时重新计时，旧滚动位置与 `--lyrics-edge-*` 在稳定后被覆盖；在 `ImmersivePlayer.test.tsx` 或 `LyricsColumn.test.tsx` 补对应断言并运行上述测试命令
- [x] 2.4 对齐 effect 额外对 `.lyrics` 视口观察内联 `style`（`MutationObserver`，回调复用 `schedule`），吸收 `useLyricsStartOffset` 在稳定后写入/清除 `margin-top` 造成的整体位移；在 `LyricsColumn.test.tsx` 增加「晚到起始偏移后重新对齐」回归测试，并确认移除该观察时测试失败。运行 `pnpm --filter @echo/desktop exec vitest run src/features/player/LyricsColumn.test.tsx`
- [x] 2.5 对齐 effect 在表面上监听 `transitionend`（仅接受表面自身与 `.playing-cover`），覆盖被遮挡窗口合成器暂停、入场 `transform` 晚于 `settled` 计时器结束的路径；在 `LyricsColumn.test.tsx` 增加对应回归测试并运行上述测试命令

## 3. 真实浏览器验收

- [x] 3.1 扩展 `apps/desktop/e2e/a85-immersive-transition.mjs`：展开前用 `window.__echoE2E__.invoke("seek", { position: 65 })` 定位到中段；入场 settle 后断言 `.lyric-line.is-current` 完整位于 `.lyrics` 视口内且中心与 `.playing-cover` 中心偏差不超过一行歌词高度；补「暂停后重新展开」与「减少动画展开立即定位」两条断言。运行 `pnpm --filter @echo/desktop build && pnpm --filter @echo/desktop test:e2e`（A85 通过）。实现中发现 mock 桥接的 `player_control` 只认 `toggle`、不认真实运行时的 `play`/`pause`，导致暂停步骤实际仍在播放；已在 `apps/desktop/e2e/mock-bridge-handlers-playback.ts` 补齐并让 A85 在暂停/恢复未生效时失败。对齐断言改为轮询收敛（≤2px），覆盖 `scroll-behavior: smooth` 的动画过程
- [x] 3.2 确认 A9 歌词专注阅读、A13/A15 沉浸式相关场景不受影响：在同一 `pnpm --filter @echo/desktop test:e2e` 运行中检查对应段落通过，并记录实际浏览器版本与结论（macOS 27.2.1 + Google Chrome 157.0.8103.79；A9/A13/A15 全部通过）

## 4. 规格、追踪与场景登记

- [x] 4.1 把本 change 的 delta（1 个 Requirement / 5 个 Scenario）追加到 `openspec/specs/immersive-lyrics/spec.md` 末尾，保持既有 Requirement 顺序与 `IL-R12-S01..S05` 的位置派生规则
- [x] 4.2 在 `scripts/verify/scenario-commands.mjs` 为 `IL-R12-S01..S05` 填入 `VITEST_ALL` 命令与 `LyricsColumn.test.tsx` / `useImmersiveTransition.test.tsx` 的 evidence；运行 `node scripts/verify/gen-scenario-manifests.mjs --write` 与 `node scripts/verify/validate-scenario-commands.mjs`
- [x] 4.3 更新 `docs/traceability.md`：基线从 155 个 Requirement / 635 个 Scenario 改为 156 / 640，并在 `immersive-lyrics` 表追加 5 行；运行 `node scripts/verify/reconcile-scenarios.mjs` 确认 specs / traceability / manifest 三方一致
- [x] 4.4 运行 `pnpm exec openspec validate fix-immersive-lyrics-alignment --strict`，确认 proposal / specs / design / tasks 全部通过

## 5. 集成验证与归档

- [x] 5.1 运行完整前端门禁：`pnpm --filter @echo/desktop test -- --run`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop build`
- [x] 5.2 运行静态与场景门禁：`node scripts/verify/checks/prototype-css-drift.mjs`、`node scripts/verify/check-scenario-evidence.mjs`、`node scripts/verify/check-scenario-command-proof.mjs`，并用 `node scripts/verify/run-scenario.mjs IL-R12-S01` 至 `IL-R12-S05` 逐条验证（避免 `pnpm verify:scenario -- <id>` 的 `--` 参数问题）
- [x] 5.3 单独运行 `pnpm verify:governance`（不与其他 cargo / 扫描任务并行），确认治理门通过（`ok governance: CI core gate passed`，覆盖 coverage ≥90%、build purity、embedded frontend freshness、scenario evidence 640/694 等）
- [x] 5.4 在 macOS 真实窗口目检：播放中段展开、暂停展开、收起后重新展开、切歌、退出歌词专注阅读、减少动画展开；确认当前行落点正确、无可见错位或卡死，并记录浏览器/系统版本。已用可见 Chrome 157.0.8103.79（macOS 27.2.1，dpr 2，1280×860）驱动 `dist/e2e` 走完全部路径并截图到 `/tmp/echo-visual-168/`：播放中段展开 `scrollTop 216`、偏差 0；暂停展开 216/0；收起后重新展开 216/0；切歌后新当前行偏差 0；退出歌词专注阅读后 0（专注态本身以歌词视口中心为目标，与唱片中心偏差 40px 属预期）；减少动画展开即时 0
- [x] 5.5 归档本 change：确认主规格已在 4.1 合入后运行 `pnpm exec openspec archive fix-immersive-lyrics-alignment --yes --skip-specs`，再运行 `pnpm exec openspec validate --archived` 与 `pnpm verify:governance`，确认 640 场景三方一致（`reconcile: spec 640 = trace 640 = manifest 640 scenarios`）
- [x] 5.6 在独立分支提交实现与归档产物，关联 Issue #168 创建中文 PR；PR 描述记录根因、方案、验证命令与 macOS 目检结论
