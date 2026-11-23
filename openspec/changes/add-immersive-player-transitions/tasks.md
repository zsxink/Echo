## 1. 原型与运动 token

- [x] 1.1 在 `docs/prototype/echo-desktop-player.html` 的 `:root` 增加 `--motion-exit: 180ms`，并在 `docs/prototype/brand-spec.md` §5 补一行退场时长与用途说明；确认原型打开仍可运行
- [x] 1.2 在原型中为 `.now-playing-popover` 增加 `data-phase="opening|open|closing"` 的 transform/opacity 规则、封面 FLIP 变量规则、歌词专注淡化规则与 `prefers-reduced-motion` 即时规则；确认 `node scripts/extract-prototype-css.mjs` 成功
- [x] 1.3 在原型脚本中把 `openNowPlaying()` / `closeNowPlaying()` 改为显式阶段机（含计时器、反向操作、减少动画即时完成），并保持既有 Escape/焦点行为；用浏览器打开原型手工展开/收起/专注切换验证无卡死
- [x] 1.4 重新生成 `apps/desktop/src/styles/{tokens,player,responsive}.css` 并运行 `node scripts/verify/checks/prototype-css-drift.mjs`，确认生成物与原型逐字一致

## 2. 过渡状态机与生命周期

- [x] 2.1 新建 `apps/desktop/src/features/player/useImmersiveTransition.ts`，实现 `closed → opening → open → closing → closed`、rAF 推进、卸载计时器、反向操作清理与 `prefers-reduced-motion` 判定；为纯状态转换补单元测试并确认通过
- [x] 2.2 在 `ImmersivePlayer.tsx` 接入阶段 hook：用 `phase !== "closed"` 决定挂载，body 类与 `phase` 同步改到 `useLayoutEffect`，并使用 `data-phase` 暴露状态；运行 `pnpm --filter @echo/desktop test -- --run src/features/player` 确认现有沉浸测试不回归
- [x] 2.3 退场期间给表面根节点设置 `aria-hidden`、`inert` 与指针禁用，保持 `useOverlay` 以 `ui.immersiveOpen` 注册；补测试断言退场中焦点已恢复、Tab/点击不进入表面、Escape 不再命中已退场层
- [x] 2.4 把 `clearArtworkTint()` 从 `!ui.immersiveOpen` 移到 `phase === "closed"`，退场期间保留 palette；补测试断言收起动画未结束前 `--player-tint` 与 `data-artwork-tint` 未被清除
- [x] 2.5 在 `artwork.css` 增加 `player-mode-entering` / `player-mode-closing` 的已注册颜色变量过渡规则，使播放栏与表面按 `--motion-base` / `--motion-exit` 还原；补静态断言确认未改生成 CSS
- [x] 2.6 补中断测试：`opening → closing`、`closing → opening`、退场中切歌、专注中直接关闭；确认最终只有一个 `closed`、无残留 body 类/计时器，并运行 `pnpm --filter @echo/desktop test -- --run src/features/player/ImmersivePlayer.test.tsx src/features/player/useImmersiveTransition.test.tsx`

## 3. 封面 FLIP 与表面动效

- [x] 3.1 新建 `apps/desktop/src/features/player/useImmersiveCoverFlip.ts`，测量 `.mini-cover` 与 `.playing-cover` 的矩形、写入 `--flip-x/y/scale` 与过渡原点，源/目标不可测量时返回退化标记；用矩形 stub 单测中心点、尺度、边界与退化路径
- [x] 3.2 在 `ImmersivePlayer.tsx` 接入 FLIP：`opening` 首帧写初始 transform，下一帧转 `open` 归零；`closing` 反向测量并动画回当前播放区；确认只写 transform 变量，不在动画帧内读写布局
- [x] 3.3 在原型/生成 CSS 中实现表面 `opacity + translateY + scale` 与封面 `transform` 的同节奏过渡，更新 `.now-playing-info` 的入场淡化；用真实 Chromium 检查首帧封面近似 `.mini-cover`、settle 后 transform 为 `none`
- [x] 3.4 覆盖窄屏（≤760px）、中等宽度（761–980px）与宽屏（≥981px）三种布局的测量与 FLIP 退化；补浏览器断言确认无横向溢出、无固定桌面尺寸假设

## 4. 歌词专注切换

- [x] 4.1 把专注模式下封面/元信息的 `display:none` 改为不占流的 `position:absolute + opacity + visibility` 隐藏方式，并补 `--motion-base` 淡化过渡；确认歌词仍独占阅读区、底部为播放栏预留空间
- [x] 4.2 在专注切换时添加一次性 `is-refocusing` 类与歌词 opacity/位移短动画，`useLayoutEffect` 同步 `lyrics-focus-open`，并保证 `useLyricsStartOffset` 在切换后重新测量；补测试断言无瞬时 `display:none`、过渡后歌词与信息块间距正确
- [x] 4.3 关闭时若处于专注阅读，冻结专注布局直到表面卸载，避免先恢复普通布局再收起；补测试断言退场中 `lyrics-focus-open` 保持到 `closed`
- [x] 4.4 更新 `apps/desktop/e2e/a9-lyrics-focus.mjs` 的“可见”判定以识别 `visibility`/`opacity`，并运行 `pnpm --filter @echo/desktop test:e2e` 确认 A9 回归通过

## 5. 切歌连续性与封面交叉淡入

- [x] 5.1 移除 `ImmersiveBody` 的 `key={currentQueueEntryId}`，确认 `useSongDetail` / `useLyrics` / `useArtworkTint` / `useLyricsStartOffset` 在 `songId` / `infoSignature` 变化时自行重置；补测试断言切歌不重挂表面、不重播入场、旧标题/歌词不残留
- [x] 5.2 在 `ImmersiveBody` 保留上一张封面并在 180ms 内交叉淡入新封面，旧图在新图可用后卸载；无封面时回落到既有占位；补测试断言切歌期间旧图不长期残留、取色按既有节奏更新
- [x] 5.3 在真实 Chromium 中确认切歌只更新封面/元信息/歌词，`data-phase` 保持 `open`，FLIP 不重新运行；确认测试不发送额外播放命令

## 6. 减少动画与可访问性

- [x] 6.1 在阶段 hook 与 CSS 两处落实 `prefers-reduced-motion: reduce`：展开/收起/专注切换即时完成、无延迟卸载、无 `is-refocusing`；补 fake `matchMedia` 测试与 CSS 静态断言
- [x] 6.2 复核退场期间 `role="dialog"`、`aria-hidden`、`inert`、焦点恢复和 Escape 分层顺序；运行 `pnpm --filter @echo/desktop test -- --run src/app/overlays.test.tsx src/features/player/ImmersivePlayer.test.tsx`
- [x] 6.3 确认黑胶唱片旋转在减少动画下仍持续、暂停/停止仍停止；用既有 `task-12.4` 静态检查与组件测试验证

## 7. 自动化测试与浏览器验收

- [x] 7.1 新建 `apps/desktop/e2e/a85-immersive-transition.mjs`，断言展开首帧/开阔状态、封面初始矩形、退场延迟卸载与 body 类生命周期、焦点恢复、无逐帧歌词高度写入；运行 `pnpm --filter @echo/desktop build && pnpm --filter @echo/desktop test:e2e`
- [x] 7.2 在 e2e 中用 CDP `Emulation.setEmulatedMedia` 覆盖 reduce 场景，断言展开/收起即时卸载且旋转例外保持；恢复媒体设置后确认后续 A9 场景不受影响
- [x] 7.3 扩展 `apps/desktop/src/features/player/ImmersivePlayer.test.tsx` 与新增过渡测试，覆盖 5 个场景对应的生命周期与视觉类断言；运行 `pnpm --filter @echo/desktop test -- --run`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop lint`

## 8. 规格、追踪与文档

- [x] 8.1 在实现完成后把本 change 的 delta（1 个 Requirement / 5 个 Scenario）合并进 `openspec/specs/immersive-lyrics/spec.md` 的既有要求之后，保持标题顺序与场景 ID 派生规则
- [x] 8.2 在 `scripts/verify/scenario-commands.mjs` 为 `IL-R11-S01..S05` 填入可执行 command 与 evidence（组件测试或 e2e 对应断言），运行 `node scripts/verify/gen-scenario-manifests.mjs --write`
- [x] 8.3 同步 `docs/traceability.md`，运行 `node scripts/verify/reconcile-scenarios.mjs`，确认 `spec 155 requirements / 635 = trace 635 = manifest 635 scenarios` 三方一致
- [x] 8.4 更新 `docs/interface-terminology.md` 中「当前播放区」「沉浸式播放器」的过渡来源说明，并确认 `docs/prototype/brand-spec.md` 的 `--motion-exit` 与实现一致
- [x] 8.5 运行 `pnpm exec openspec validate add-immersive-player-transitions --strict`，确认 proposal/specs/design/tasks 全部通过

## 9. 集成验证与归档

- [x] 9.1 运行 `node scripts/verify/checks/prototype-css-drift.mjs`、`node scripts/verify/validate-scenario-commands.mjs`、`node scripts/verify/check-scenario-evidence.mjs`、`node scripts/verify/check-scenario-command-proof.mjs`，确认生成物与场景命令有效
- [x] 9.2 运行完整前端门禁：`pnpm --filter @echo/desktop build`、`pnpm --filter @echo/desktop test -- --run`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop format:check`
- [ ] 9.3 运行 `pnpm verify:governance` 与 `pnpm verify:scenario -- --all`，确认完整治理与场景门禁通过
- [ ] 9.4 在 macOS、Windows、Linux 各执行一次展开/收起/专注切换/切歌/减少动画人工检查，记录平台、WebView 版本与结论；确认无闪烁、错位、卡死或性能回退
- [ ] 9.5 归档本 change，确认新 Requirement 回到主规格、635 场景三方一致，`pnpm exec openspec validate --archived` 通过
