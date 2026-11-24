## Context

Issue #168 的根因在 `apps/desktop/src/features/player/LyricsColumn.tsx` 的滚动对齐 effect：它用 `getBoundingClientRect()` 求「当前行中心 → 目标高度」的差值并写入 `scrollTop`，同时写入 `--lyrics-edge-top/bottom`。宽屏布局的目标高度取 `.playing-cover` 的中心。

#85 之后，表面挂载时处于 `data-phase="opening"`：

- `apps/desktop/src/styles/player.css` 给 `.now-playing-popover[data-phase="opening"]` 加 `translateY(10px) scale(.985)`，给 `.playing-cover` 加 FLIP 的 `translate(...) scale(...)`；后者的位移来自常驻播放栏 `.mini-cover`，尺度约为 `48 / 424`（钳到 0.2）。
- `apps/desktop/src/features/player/useImmersiveTransition.ts` 在 `opening → open` 只等两帧，但 CSS 过渡仍持续 `--motion-base`（220ms）。
- `useLyrics` 在挂载时先清空再异步拉取，歌词通常在这 220ms 内到达，使对齐 effect 恰好在 FLIP 进行时重跑。
- 对齐 effect 的依赖是 `[currentLineIdx, autoScroll, focusOpen, lyrics]`；`ResizeObserver` 观察 border-box 尺寸，`transform` 不改变布局尺寸，因此过渡结束不会自动纠正。暂停时 `currentLineIdx` 不变，错误位置会一直保持。

第二个、独立的竞态来自 `useLyricsStartOffset`：它在宽屏下给 `.lyrics` 写内联 `margin-top`（信息块高度 + 20px，实测约 215px），触发路径包括 `fonts.ready` 与双帧 settle，冷启动时可能落在入场 settle 之后。该写入整体下移歌词视口与所有歌词行，但不改变任何被 `ResizeObserver` 观察的 border-box 尺寸，也不改变对齐 effect 的依赖，因此对齐结果会停留在按旧布局算出的位置。目检（Chrome 157，`/tmp/echo-visual-168`）显示：冷启动后首次播放中展开 `scrollTop 1`、当前行偏离唱片中心 216px；关闭后再次展开（margin 已写入）则 `scrollTop 215`、偏离 1px，指向「晚到的 margin 写入未被纠正」而非过渡几何本身。

## Goals / Non-Goals

**Goals:**

- 展开/重新展开/从退场中断恢复后，歌词列只在表面与封面的入场 `transform` 过渡结束后按稳定几何对齐一次。
- `opening` / `closing` 期间不依据被 `transform` 扭曲的矩形写入 `scrollTop` 或 `--lyrics-edge-*`。
- 歌词晚到、切歌、退出歌词专注阅读后仍能按当时的稳定几何重新对齐；暂停展开也生效。
- 宽屏下 `useLyricsStartOffset` 在稳定后写入/清除内联 `margin-top` 时，歌词列按新几何重新对齐。
- 减少动画下立即对齐；不改变播放、当前行判定、seek、手动滚动暂停规则。

**Non-Goals:**

- 不改变过渡时长、FLIP 算法、阶段机状态集合或视觉设计。
- 不改写歌词来源、LRC 解析、`useLyricsStartOffset` 的信息块起点公式。
- 不把对齐改成逐帧动画、平滑滚动库或布局坐标重写（方案 2 的更大改动留待未来需要时评估）。
- 不新增跨层依赖；只改 Presentation 层的沉浸式播放器与歌词列。

## Decisions

### 1. 用派生的 `settled` 信号，而不是在歌词列里重复阶段逻辑

在 `useImmersiveTransition` 的返回值中增加 `settled: boolean`：

- `phase === "opening"` / `"closing"` 时为 `false`；
- `phase === "open"` 时，等待 `IMMERSIVE_OPEN_MS`（镜像 `--motion-base`）后置为 `true`；
- `prefers-reduced-motion: reduce` 时在进入 `open` 的同一提交内即为 `true`；
- `closed` 时为 `false`。

理由：阶段机已经拥有「入场/退场何时结束」的知识；歌词列只需要知道「周围几何是否稳定」，不需要理解 `opening/open/closing`。这样 `LyricsColumn` 的输入是一个与滚动语义相关的布尔值，而不是把沉浸式阶段机耦合进歌词组件。

替代方案：

- **只等 `phase === "open"`**：不足够。`open` 只表示阶段机推进，CSS 过渡仍有 220ms，封面仍在 FLIP 后的回归过渡中。
- **`transitionend` 过滤 `transform`**：更贴近真实结束时刻，但需要处理目标元素、冒泡、`transitioncancel`、无过渡（退化路径）和 jsdom 不触发事件等分支；阶段机已经用计时器镜像 CSS 时长（退场卸载就是同一模式），继续用计时器更一致、可测。
- **在歌词列内部监听 phase**：会重复阶段时序知识，且未来新增使用方时容易分叉。

`settled` 的计时器在 `phase` 变化时清理；`closing → open` 的中断恢复也会重新等待一个 `--motion-base`，因为表面正从退场 transform 过渡回 `none`。

### 2. `LyricsColumn` 在几何未稳定时完全跳过对齐

给 `LyricsColumn` 增加 `geometrySettled: boolean` 输入（由 `ImmersiveBody` 透传），并把 `geometrySettled` 加入对齐 effect 的依赖：

- `geometrySettled === false`：effect 直接返回，不创建 `ResizeObserver`、不排 rAF、不写 `scrollTop` / `--lyrics-edge-*`；
- `geometrySettled === true`：保持现有对齐算法与 rAF 时序，读取的封面/表面矩形已不再被入场 `transform` 扭曲；
- 歌词晚到、切歌、`focusOpen` 变化时 effect 本来就会重跑；`geometrySettled` 变化会补上「入场结束后」这一条触发路径。

不对 `scrollTop` 做清零或动画：跳过写入即可。表面退场时歌词随表面淡出；重新展开时表面会重新挂载（或从 `closing` 恢复），稳定后一次写入覆盖旧值。

### 3. 保持现有目标公式，不重写为布局坐标

宽屏仍用 `cover.top + cover.height / 2`，窄屏/专注仍用歌词视口中心。稳定后 `data-phase="open"` 且封面 `transform: none`，`getBoundingClientRect()` 与布局坐标一致，因此无需引入 `offsetTop` 链路或临时中和 `transform` 的读取（方案 2/3）。

代价是 `settled` 由计时器而非 `transitionend` 决定：若未来 CSS 时长改变而常量未同步，会早/晚对齐。用现有 `immersiveTransitionStyles.test.ts` 的静态契约与常量镜像约束，并在实现任务中核对 `--motion-base` 与 `IMMERSIVE_OPEN_MS` 一致。

### 4. 减少动画与中断路径

- `prefers-reduced-motion: reduce`：阶段机直接 `open`，`settled` 初始/同步为 `true`，歌词列在挂载后的第一帧对齐，不等待计时器。
- `opening → closing`：`settled` 保持 `false`，对齐 effect 清理，不写滚动。
- `closing → open`：`settled` 重新等待 220ms；表面从当前视觉状态回到 `open`，歌词在稳定后对齐一次。
- 退场中切歌：`settled` 为 `false`，不写滚动；重新展开后按新歌曲对齐。
- 专注阅读切换：布局本身立即重排，`settled` 已为 `true`，对齐 effect 按既有路径重跑；`is-refocusing` 的 4px 一次性位移可能带来不超过一行高度的偏差，在规格容差内，不额外等待。

### 5. 对齐 effect 额外观察歌词列的 `style` 属性与表面的 `transitionend`，吸收晚到的布局变化

`LyricsColumn` 的对齐 effect 在创建 `ResizeObserver` 的同时：

- 对 `.lyrics` 视口创建一个 `MutationObserver`（`attributes: true, attributeFilter: ["style"]`），回调复用同一个 `schedule`（取消旧帧、重排一帧 `align`）；
- 在表面上监听 `transitionend`，仅接受目标为表面自身或 `.playing-cover` 的事件，同样复用 `schedule`；
- cleanup 一并 `disconnect` 与 `removeEventListener`。

理由：`useLyricsStartOffset` 的写入是**内联样式**，移动的是视口自身与全部歌词行；`ResizeObserver` 观察 border-box 尺寸，尺寸不变则沉默，effect 依赖也不变。观察 `style` 是能覆盖该写入路径的最小通道，而且不需要让歌词列知道起始偏移的公式或信息块。`settled` 计时器镜像 CSS 时长，但被遮挡的窗口可能暂停合成器，入场 `transform` 晚于计时器结束；此时封面/表面的 `transitionend` 是唯一还会到达的信号，重跑一次对齐即可收敛。两条路径都是幂等的，正常路径只多一次廉价的矩形读取。

替代方案：

- **在 `useLyricsStartOffset` 里回调歌词列**：需要把两个 hook 用 ref/回调串起来，或把偏移值提为共享状态，耦合与改动面都更大；且歌词列本来就有「几何变化后重对齐」的职责。
- **延长 `settled` 等待（如 fonts.ready 后再等）**：`fonts.ready` 只覆盖其中一条触发路径（还有双帧 settle、resize、media query 变化），且会把对齐推迟到不确定时刻。
- **忽略该竞态，只靠 A85**：A85 在 mock 桥接里字体即时可用，未能复现；真实 WebView/冷启动仍会错位，属于用户可见缺陷。

对齐 effect 只写 `--lyrics-edge-*` 到 `.lyrics-track`，不写视口自身的 `style`，因此该观察不会形成自触发循环。

## Risks / Trade-offs

- [入场期间歌词列可能先显示旧/顶部位置，稳定后一次跳正] → 表面同时在淡入，且旧实现也有「歌词异步到达后一帧对齐」的同类过渡；规格只要求过渡结束后当前行已定位。若目检认为跳变明显，后续可评估在 `opening` 首帧用 FLIP 已测得的目标矩形预对齐（方案 2 的局部化），不在本 change 扩大范围。
- [`settled` 计时器与 CSS 时长漂移] → 常量已镜像 `--motion-base`，在静态样式测试与实现任务中核对；计时器只影响对齐时机，不影响目标几何。
- [`closing → open` 时过早对齐] → `settled` 在每次进入 `open` 时重新计时，覆盖中断恢复路径；组件测试覆盖该路径。
- [专注切换的 4px 一次性位移] → 目标容差为一行歌词高度；组件测试用稳定几何断言，不把 `is-refocusing` 中间帧当作落点。
- [对齐写入触发平滑滚动] → `.lyrics` 的 `scroll-behavior: smooth`（`app-extras.css`）会让一次对齐写入在数帧内动画到目标，真实浏览器断言因此轮询等待收敛（≤2px），而不是固定 sleep；`prefers-reduced-motion: reduce` 下该规则被 `scroll-behavior: auto !important` 关闭，仍是即时定位。
- [jsdom 无布局、无 `matchMedia`] → 组件测试继续用矩形/`matchMedia`/fake timer stub；真实几何由 A85 的 Chromium 断言补足。
- [新增场景改变追踪基线] → 实现完成时统一登记 `IL-R12-S01..S05`，运行 manifest 生成与 traceability 对账，不在 propose 阶段提前写入。

## Migration Plan

1. 本 change 只落规划产物；主规格、追踪表与 manifest 保持当前基线。
2. 实现阶段先改 `useImmersiveTransition` 的 `settled` 派生与 `LyricsColumn` 的输入，再给对齐 effect 补上对 `.lyrics` 内联 `style` 的观察，最后补组件测试与 A85 浏览器断言。
3. 实现完成后把 delta 的 5 个场景登记到 `scripts/verify/scenario-commands.mjs`，运行 `gen-scenario-manifests.mjs --write` 与 `docs/traceability.md` 同步，确认三方集合一致。
4. 归档：把新 Requirement 合并回 `openspec/specs/immersive-lyrics/spec.md`，运行 `openspec validate --archived` 与治理门禁。
5. 回滚：改动集中在前端组件、hook 与测试，无持久化/IPC 迁移；`git revert` 单个实现提交即可恢复「入场中即对齐」的旧行为。
