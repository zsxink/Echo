## Context

Issue #85 要求沉浸式播放器的进入/退出以及歌词专注阅读切换具有连续过渡。当前实现的事实：

- `apps/desktop/src/features/player/ImmersivePlayer.tsx` 在 `ui.immersiveOpen` 为假时直接 `return null`；`ImmersiveBody` 以 `snapshot.currentQueueEntryId` 为 `key`，切歌会重挂整个表面。`player-mode-open` / `lyrics-focus-open` 由 `useEffect` 增删（提交后生效）。
- 沉浸面与常驻播放栏的实际配色不是原型中的深色：`apps/desktop/src/styles/artwork.css` 作为应用自有覆盖层，把 `--player-background` / `--player-glow` / `--player-on` 改为封面取色派生的浅色「素白」表面，并用已注册颜色变量的 800ms 过渡处理切歌取色。生成物 `styles/{tokens,player,responsive}.css` 来自 `docs/prototype/echo-desktop-player.html`，禁止手改。
- `.now-playing-popover` 目前没有进入/退出动画；`.playing-cover` 是 48px 常驻播放栏 `.mini-cover` 之外的独立 176–424px 唱片。歌词专注阅读通过 `display:none` 与整组 grid 参数替换完成。
- 浮层语义集中在 `apps/desktop/src/app/overlays.ts`：`useOverlay` 按 `ui.immersiveOpen` 注册 `Immersive` / `LyricsFocus` 两层，负责 Escape 优先级、焦点进入与关闭后恢复；`useFocusTrap` 负责 Tab 圈定。收起按钮 portal 到 body，所以不能只靠容器内的 DOM 判断可用性。
- `useArtworkTint` 在外层 `useLayoutEffect` 中于 `!ui.immersiveOpen` 时立刻 `clearArtworkTint()`；若直接用于退场，会在动画第一帧把取色退回主题色。
- 全局 `prefers-reduced-motion: reduce` 规则（`styles/responsive.css`）把 transition/animation 压到 1ms，但不会取消 JS 的延迟卸载，所以降级必须同时发生在生命周期层。

## Goals / Non-Goals

**Goals:**

- 进入、退出、歌词专注切换与切歌都有连续、克制、可解释的视觉过渡。
- 不改变播放、队列、歌词同步、seek、Escape 分层、焦点陷阱和 `role="dialog"` 语义。
- 过渡可中断，任何中途操作都能收敛到唯一稳定状态。
- 只动画合成友好属性；减少动画下即时完成；三平台 WebView 行为一致。

**Non-Goals:**

- 不追求逐帧物理模拟、液体形变、模糊半径动画或第二套材质系统。
- 不在本 change 重写沉浸面视觉、歌词数据流或浮层栈。
- 不引入截图基线作为唯一门禁；自动验证以真实 Chromium 的计算布局/样式与组件生命周期为主，三平台以人工视觉核对补足。

## Decisions

### 1. 用显式阶段机，而不是 CSS 离散显隐

采用 `closed → opening → open → closing → closed` 的显式阶段，而不是 `transition-behavior: allow-discrete` + `@starting-style` 或 `hidden` 属性过渡。后两者在 macOS WebKit、Windows WebView2 与 Linux WebKitGTK 的版本分布不一致，且 `hidden` 的可访问性语义要求立即移除，无法同时满足“退场可见”。阶段机由 React 持有，退场用定时器延迟卸载；`prefers-reduced-motion` 时不进入 `opening/closing`，直接切换。

新增 `apps/desktop/src/features/player/useImmersiveTransition.ts`：输入 `open: boolean`，输出 `phase`，并负责清理计时器、处理反向操作（收起中再展开）与减少动画判定。`opening → open` 在下一帧推进，让首帧的 FLIP 初始状态先被绘制。阶段通过 `data-phase` 暴露给样式与测试。

### 2. React 拥有生命周期，CSS 只拥有视觉

职责拆分：

- **React / 阶段 hook**：何时挂载、何时退场、body 类、覆盖层注册、焦点、`aria-hidden`/`inert`、计时器、切歌与中断。
- **原型 + 生成 CSS**：`data-phase` 对应的 transform/opacity/颜色过渡、动画时长和减少动画规则。
- **`artwork.css`（应用自有）**：浅色沉浸面的颜色变量过渡、播放栏与表面的主题还原，不修改生成物。

body 类在 `useLayoutEffect` 中根据 `phase` 更新，避免提交后一帧才生效造成的闪变：

- `phase !== "closed"` → `player-mode-open`；
- `phase === "opening"` → `player-mode-entering`（把已注册颜色变量的过渡压到入场节奏）；
- `phase === "closing"` → `player-mode-closing`（保持表面可绘制，同时把颜色变量过渡到普通主题）；
- `phase === "closed"` → 全部移除，并在此时才 `clearArtworkTint()`。

`useOverlay` 的 `enabled` 继续绑定权威的 `ui.immersiveOpen`，不用 `phase`。因此关闭动作一发生，浮层立即退出栈、Escape 不再命中它、焦点恢复按现有 `useOverlay` 语义执行；表面只是视觉上继续存在 180ms。退场期间给表面根节点设置 `aria-hidden="true"` 与 `inert`（通过 ref 写入，兼容 React 18），并禁用指针事件。

### 3. 封面 FLIP + 表面淡入，避免整屏缩放

入场：

1. 表面挂载后、绘制前，测量 `.playerbar .mini-cover` 的视口矩形与 `.now-playing-popover .playing-cover` 的最终布局矩形。
2. 由两个中心点与宽度比得到 `--flip-x` / `--flip-y` / `--flip-scale`，写入 `.playing-cover`；`data-phase="opening"` 下对 `.playing-cover` 应用 `translate(...) scale(...)`，`transform-origin: center`。
3. 下一帧切到 `open`，CSS 在同一 `--motion-base` 内把封面从该变换过渡到 `none`；表面本身同时 `opacity: 0 → 1`、`translateY(10px) → 0`、`scale(.985) → 1`，`transform-origin` 指向当前播放区中心（由阶段 hook 写入 `--transition-origin-x/y`），形成“从播放栏生长”的方向感。
4. 只动画 `transform` 与 `opacity`；唱臂继续使用既有 480ms 曲线，不因入场重新触发。封面 FLIP 结束前后，唱臂与唱片细节已由表面淡入覆盖，不额外做逐帧滤镜。

当 `.mini-cover` 不存在、宽高为 0 或目标不可测量时，跳过 FLIP，仅保留表面淡入；窄屏布局同样按实际目标矩形计算，不写死桌面尺寸。

退出反向执行：关闭开始时测量当前播放区与唱片最终布局矩形，写入同一组变量，`data-phase="closing"` 下过渡到该变换；表面淡出。若在入场未完成时关闭，CSS transition 会从当前计算出的 transform 继续重定向到退场目标；不强制清空 transform，避免跳帧。

不采用 `clip-path` 圆形揭示或整屏 `scale(0.2 → 1)`：前者逐帧重绘全屏层，后者会让文字和歌词在放大过程中失真，均不符合“不引入性能回退”和克制的目标。

### 4. 颜色、材质与取色连续

- 入场时 `player-mode-entering` 把 `--player-tint` / `--player-background` / `--player-glow` / `--player-on` 的过渡时长从切歌用的 800ms 收到 `--motion-base`；表面淡入与取色落位同步。
- 退场时保留当前 palette 进入 `closing`，`player-mode-closing` 把这些变量按 `--motion-exit` 过渡回普通主题值（`--player-background: var(--bg)` 等）。已注册颜色变量会在 body 上插值，播放栏背景、表面底色、文字层级随表面淡出同时收敛，不会出现“表面先消失、播放栏后跳色”。
- 取色清理从 `!ui.immersiveOpen` 移到 `phase === "closed"`；否则退场第一帧就会退回主题色。
- 不主动动画 `backdrop-filter`。常驻播放栏的玻璃观感由颜色变量与既有 CSS 承担，避免跨平台模糊过渡差异。
- 切歌时继续使用 `useArtworkTint` 的已注册变量过渡；封面本身用“保留上一张、短交叉淡入”的方式更新，与入场 FLIP 解耦。

### 5. 歌词专注切换：淡化 + 歌词重新落位

歌词专注阅读是同一表面的重排，不接受对 `grid-template-areas` 或 `display` 做逐帧动画。采用：

- 封面与元信息从 `display:none` 改为可过渡的隐藏方式（`position:absolute` + `opacity:0` + `visibility:hidden` + `pointer-events:none`）；退出专注时可见性立即恢复、透明度在 `--motion-base` 内淡入。保持它们不在 grid 流内，歌词仍独占阅读区。
- 歌词区域在专注切换时加一次性 `is-refocusing` 类，播放一段 opacity/4px 位移的短动画，把即时重排藏在同一节奏内；不改变字号、内边距或高度的逐帧值，继续由 `useLyricsStartOffset` 在 `focusOpen` 变化后重新测量。
- 进入/退出专注仍由 `ui.focusOpen` 驱动；Escape 顺序不变。`body.lyrics-focus-open` 也改在 `useLayoutEffect` 中更新，防止“先按普通布局绘制一帧再跳”。
- 关闭播放器时若当前在专注阅读，退场阶段冻结专注布局（保留 `lyrics-focus-open` 到卸载），避免先展开封面/元信息再收起造成二次重排。

### 6. 切歌不重挂表面

移除 `ImmersiveBody` 上按 `currentQueueEntryId` 的 `key`。`useSongDetail`、`useLyrics`、`useArtworkTint`、`useLyricsStartOffset` 都以 `songId` / `infoSignature` 为输入自行重置，`failedCoverId` 已按 `songId ?? coverKey` 区分，因此不需要整面重挂。收益：

- 入场动画不会在每次切歌时重播；
- 专注布局、滚动容器和 surface 引用保持稳定，FLIP 目标可继续测量；
- 封面用两层 `<img>`（上一张 + 当前张）在 180ms 内交叉淡入，旧图在新图 decode 后卸载；无封面时回落到既有占位。

### 7. 减少动画与中断

- 阶段 hook 每次开始过渡时读取 `window.matchMedia("(prefers-reduced-motion: reduce)").matches`（jsdom 无该 API 时视为 false，并允许测试注入）。为 `true` 时：展开直接 `open`、收起直接 `closed`、专注切换不添加 `is-refocusing`，不设置任何延迟卸载计时器。
- CSS 侧继续依赖现有全局 1ms clamp，并新增针对 `data-phase` 的 `transition: none`，确保即使媒体查询在过渡中变化也不会留下半程状态。
- 中断路径：
  - `opening + close`：清除 `opening → open` 的 rAF，进入 `closing`；CSS 从当前 computed transform 继续；
  - `closing + open`：清除卸载计时器，重新进入 `opening` 并保留当前表面节点；
  - 退场中切歌：只更新数据，不改变 phase；`data-phase="closing"` 继续；
  - 任何路径结束时都只允许一个 `closed`，body 类与计时器由同一个 effect 清理。

### 8. 运动 token 与原型管线

在原型 `:root` 增加 `--motion-exit: 180ms`，作为退场专用 token；入场与专注切换复用 `--motion-base`，切歌取色复用 800ms 既有节奏，微交互保持 `--motion-fast`。同步更新 `docs/prototype/brand-spec.md` §5 的时长表。`tokens.css`、`player.css`、`responsive.css` 由 `node scripts/extract-prototype-css.mjs` 生成，`artwork.css` 的应用自有颜色过渡手写并加注释说明为何不放进生成物。

### 9. 验证策略

- **组件测试**（jsdom）：`ImmersiveTransition.test.tsx` 使用 fake timers、rAF stub、`matchMedia` stub 与 `getBoundingClientRect` stub，断言 phase 生命周期、body 类、`aria-hidden`/`inert`、焦点恢复、中断收敛、减少动画零延迟；扩展 `ImmersivePlayer.test.tsx` 覆盖切歌不重挂、专注切换类与残影。
- **真实浏览器**：新增 `apps/desktop/e2e/a85-immersive-transition.mjs`，在现有 mock-bridge Chromium harness 中断言首帧存在 `data-phase="opening"`、封面初始矩形贴近 `.mini-cover`、settle 后 `data-phase="open"` 且 transform 归零；关闭后表面在退场窗口内仍在 DOM、`aria-hidden`/`inert` 生效、body 类保留，超时后全部移除；用 CDP `Emulation.setEmulatedMedia` 验证 reduce 下即时卸载；断言动画期间歌词容器高度/滚动位置不被逐帧改写。
- **静态门禁**：`prototype-css-drift` 保证生成物与原型一致；测试读取 CSS 断言只对 `transform`/`opacity`/已注册变量设置过渡，且 `prefers-reduced-motion` 规则存在。
- **人工视觉复核**：macOS WKWebView、Windows WebView2、Linux WebKitGTK 各执行一次展开/收起/专注/切歌/减少动画检查，在 PR 记录平台与结论；不把截图作为唯一证据。

## Risks / Trade-offs

- [延迟卸载可能与“关闭即不可交互”冲突] → 关闭时立即退出浮层栈并设置 `inert`/`aria-hidden`，焦点在退场开始即恢复；组件测试显式断言退场期间 Tab/点击不进入表面。
- [FLIP 在窗口移动、DPI 变化或布局切换时坐标失准] → 每次展开/收起重新测量；源或目标不可测量即退化为淡入；不缓存跨窗口矩形。
- [移除整面 `key` 可能暴露旧数据的短暂可见] → `useSongDetail` / `useLyrics` 在 `songId` 变化时立即清空，封面交叉淡入由显式上一张状态控制；测试覆盖“切歌后不残留旧标题/封面/取色”。
- [歌词专注把 `display:none` 改为 absolute/visibility 会影响现有 A9 断言] → 同步更新 `a9-lyrics-focus.mjs` 的可见性判定，并继续断言高度、当前行和底部播放栏空间。
- [已注册颜色变量在退场中回落到普通主题，可能与表面透明度叠加成“发白”] → 退场时长仅 180ms，且表面同步淡出；e2e 采样中段颜色与最终颜色，必要时只回退 `--player-background` 而不改 `--player-tint`。
- [三平台 WebView 对 `inert`、`visibility` 过渡、`@property` 的支持差异] → 不使用 `allow-discrete`；`inert` 用属性写入并保留 `aria-hidden` + 指针禁用作为兜底；FLIP 与颜色过渡都有非动画降级路径。
- [新增 1 个 Requirement / 5 个 Scenario 会改变场景基线] → 在实现完成时统一登记，不在 propose 阶段提前写入 `traceability.md` / `manifest.json`；归档前运行 reconciliation 与完整治理门禁。

## Migration Plan

1. 本 change 只落规划产物；主规格、追踪表与 manifest 保持当前 153 / 630 基线。
2. 实现阶段先改原型并重生成 CSS，再落 React 阶段机与 FLIP，最后补测试与文档。
3. 实现完成后把 delta 的 5 个场景登记到 `scripts/verify/scenario-commands.mjs`，运行 `gen-scenario-manifests.mjs --write` 与 `docs/traceability.md` 同步，使三方集合在 154 / 635 上一致。
4. 归档：`openspec archive` 把新 Requirement 合并回 `openspec/specs/immersive-lyrics/spec.md`，再运行 `openspec validate --archived` 与 `pnpm verify:governance`。
5. 回滚：改动集中在前端样式、原型与测试，无持久化/IPC 迁移；`git revert` 单个实现提交即可恢复硬切换行为。
