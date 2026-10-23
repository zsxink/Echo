# Design

## 根因

宽屏沉浸布局（`player.css` ≥981px 媒体查询）把 `.now-playing-info` 与 `.lyrics` 放进同一 grid area，信息块 `z-index: 1` 叠在上层；两者唯一的分隔是 `useLyricsStartOffset` 在 `useLayoutEffect` 里测出的 `--lyrics-start-offset`（`info.bottom - lyrics.top + 20px`）。该测量的触发键是 `infoSignature = title|artist|album|duration` 与 `window.resize`。自动切歌按 `currentQueueEntryId` 重挂载整个 `ImmersiveBody`，重挂载当次测量的时机由 WebView 的字体与 IPC 时序决定：Windows 上测量落在信息块达到最终高度之前（CJK 回退字体度量、`song_detail` 迟到、折行差异），而此后四个签名字段不再变化，没有任何机制再触发重算——歌词从此画进信息块。macOS 恰好测量得晚，所以不复现。这是一类问题（高度变化不出现在签名里），不是单一时序点。

## 方案：把「按签名测量」改成「按几何观测」

`useLyricsStartOffset` 对外签名不变（`shellRef` + `infoSignature`），内部改为多通道重测：

1. **`ResizeObserver` 观察 `.now-playing-info`**（核心）。高度是唯一事实来源：不枚举成因（字体、明细、折行、重挂载），高度变了就重测。信息块是 `align-self: start` 的普通流元素，`apply()` 只写歌词列的 `margin-top` 自定义属性，不改变信息块尺寸，无观察反馈环。
2. **初次测量保持在 `useLayoutEffect` 同步执行**。打开沉浸模式的既有路径（e2e A9 首次断言、macOS 行为）依赖它在首帧前生效，不能推迟。
3. **双 `requestAnimationFrame` 后补测一次**：重挂载后的第一轮布局可能携带回退字体度量，两帧后字体通常已替换；补测由同一 `apply()` 完成，幂等。
4. **`document.fonts.ready` 后补测一次**：WebView2 字体时序是本次 Windows 必现的主因，`fonts.ready` 是「最终字体度量已生效」的明确信号；即使 RO 因高度恰好不变而未触发（度量变了但高度没变时偏移公式本就不变），这次补测也无害。
5. **保留 `window.resize` 监听**与「先 `removeProperty` 再测量」的方式（属性移动被测对象自身，必须先摘掉）。

降级：jsdom 与老 WebView 逐项探测（`typeof ResizeObserver === "function"`、`document.fonts?`、`matchMedia` 沿用 `isWideLayout` 的既有降级），缺哪个就少一条通道，测量本身照旧。

清理：rAF 取消、RO disconnect、监听移除、`fonts.ready.then` 以 disposed 标志拦截，防止重挂载后旧回调写新 DOM。

## e2e 门禁：让切歌路径在 CI 可见

- `mock-bridge.ts` 的 `player_control: next` 当前是 `position + 10` 的占位（无任何现有断言依赖），改为与真实运行时一致的切歌语义：推进到队列下一首（循环）、position 归零、保持播放态，并按既有机制重发快照（`currentQueueEntryId` 变化 → 前端整块重挂载，正是自动切歌走的路径）。`snapshotOf` 补齐 `currentArtist`/`currentAlbum`（真实快照有这两个字段，重挂载后首帧元信息不再是「未知艺人」，也与 Windows 上「明细迟到补全」的时序更接近）。
- `run-e2e.mjs` A9 在既有打开断言之后：
  1. 点击播放栏「下一首」→ 等待重挂载与两帧 → 再次断言 `lyricsTop ≥ infoBottom` 且 `--lyrics-start-offset` 非零；
  2. **判别步**：向信息块注入一次与签名无关的高度变化（`style.paddingBottom`），等待 RO 送达与重测 → 断言歌词仍让开信息块；移除注入再断言回落。旧实现（只有签名 + resize）在这一步必红——这正是「高度变化不出现在签名里」的失效类在浏览器层的可复现形式（字体竞态本身无法跨平台确定性复现，注入高度变化是对同一失效类的确定性模拟）。

## 单元层

新增 `useLyricsStartOffset.test.tsx`（jsdom stub `matchMedia` 宽屏命中、`ResizeObserver` 捕获实例与回调、两个元素的 `getBoundingClientRect` 可变）证明三条通道：初始/layout-effect 测量、RO 回调按新高度重算、`resize` 事件重算；并证明签名变化重建观测。几何本身在 jsdom 无法成测，故由上述 e2e 断言真实几何。

## 被否决的替代方案

- **扩充 `infoSignature`**（把明细加载态、字体状态拼进签名）：枚举成因永远追不完，正是本次根因；几何观测一次覆盖全部成因。
- **纯 CSS（`.lyrics { margin-top: … }` 固定值或 `min-height`）**：偏移依赖信息块渲染高度（折行、字体、明细），样式表拿不到这个输入；固定值在折行场景下重新交叠。
- **给 `ImmersiveBody` 加更多 `key`/重挂载粒度**：不解决「测量晚于最终高度」的时序问题，只换一个重挂载点。
- **测量全部推迟到双 rAF / `fonts.ready` 之后**：打开表面的首帧失去偏移（歌词闪现在标题下方），回归既有 A9 首次断言的时序前提。

## 影响边界

只动桌面前端测量 hook、e2e mock 与门禁；Presentation 内部行为修正，不改 IPC 契约、不改 Core、不改窄屏布局。与待定 change `align-immersive-player-and-playlist-directory` 的 IL delta（布局/主题/歌单目录要求）无文本重叠：本 change 的 requirement 表述「持续对齐契约」（何时必须重算），其 delta 表述「布局与主题的静态要求」，两者归档后共存于主规格，编号按各自归档顺序顺延。
