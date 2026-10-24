# Design

## 根因：清除旧偏移与读取几何之间隔着一帧布局

宽屏沉浸布局把 `.now-playing-info` 与 `.lyrics` 放进同一 grid area，两者起点都是区域顶端（`areaTop`），歌词列只靠 `margin-top` 下移。偏移的几何定义因此是

```
offset = infoBottom − areaTop + gap
```

旧实现按规格字面「测量必须先移除旧偏移再读取几何」执行：

```ts
lyrics.style.marginTop = "";
const offset = info.bottom - lyrics.top + gap;   // 期望 lyrics.top === areaTop
```

问题在第二行。探针在 A9「信息块增长」一步记录到同一次测量内的两次读取：`info.bottom = 217`、`lyrics.top = 189`，而当时内联 margin 是 `109px`、`areaTop` 是 80——`lyrics.top` 返回的是**旧偏移所在的位置**，清除尚未反映到布局。于是

```
offset = 217 − (80 + 109) + 20 = 48
```

写入 48px 后歌词落到 128px，压在信息块（底部 217px）之内。每一趟重驱都重复同一读数，3 趟后停下，界面长期停在塌缩值上——用户看到的「歌词偶尔画进歌曲名」正是这一类。这不是探针特有：WebView2 与经 `Emulation.setDeviceMetricsOverride` 驱动的 Chromium 都出现同样的「样式已改、布局晚一两帧」，同一环境里也观测到 `getComputedStyle` 晚于内联声明生效。

## 方案：把偏移定义成信息块的高度

`areaTop` 不可靠地读不到，但**信息块的高度**可以：信息块是本 hook 从不移动的盒子，它自己的 rect 不会对 hook 刚写的 margin 过期。由定义

```
offset = infoBottom − areaTop + gap = infoHeight + gap      （两盒起点同为 areaTop）
```

`apply()` 只读信息块的 `height` 求值，再把歌词列**当前已生效的偏移**以算术方式扣除，得到未偏移位置，仅用于判断「两盒是否共享同一区域」：

```ts
const applied = parseFloat(lyrics.style.marginTop) || 0;
const naturalTop = lyricsRect.top - applied;
if (naturalTop >= infoRect.bottom + gap - tolerance) return;  // 已让开：不是共享区域
lyrics.style.marginTop = `${Math.round(infoRect.height + gap)}px`;
```

三个性质：

1. **不依赖清除是否可见**：值只来自信息块，陈旧布局读到的歌词位置最多影响「是否共享区域」的判断，不影响数值。
2. **幂等**：不再为测量清除 margin，连续多趟得到同一个数，观测通道可以随便重复触发（双 rAF、RO、字体、缩放、断点）。
3. **保留失效保护**：信息块无渲染盒（`display: none`）时 `hasRenderedBox` 直接拒绝，上一趟合法值保留；两盒不共享区域时以 `naturalTop` 判定并退出，不会把「已经让开」的距离再叠加一次。

写入后的几何判定保留（判定写入是否真的生效），在布局滞后的环境里会多驱动一帧，然后回落——值本身已经是正确的，重驱只是确认。

## 被否决的替代方案

- **把「清除 → 读取」拆成两帧**（第一帧清除，下一帧读取并写入）：能绕过陈旧读，但每趟测量都改两次被测元素，观测通道（尤其 `ResizeObserver`/`resize`）会和自身的写入互相触发；而且需要把「等待布局」写进 hook 的状态机，`useLayoutEffect` 的首帧语义（打开沉浸模式时不能闪）也要重定义。
- **在 hook 内记住「我上次写了多少」并用它扣除**：与内联值重复记账，任何一处外部清除（专注模式、断点切换、样式回退）都会让账本漂移。
- **对 `.lyrics` 挂 `ResizeObserver`**：写入 margin 会改变它自身尺寸，观察回调与写入形成反馈环；而信息块的几何已经足够。
- **提高重驱次数 / 加固定 `setTimeout`**：把「测错」变成「多测几次还是错」，并且拖慢打开路径。

## 门禁：断言几何，而不是断言写入值

旧 A9 断言的是 `margin-top` 的值（`=== "0px"`）。这个断言在两类真实故障上都失明：

- **塌缩值不是 0**：无盒测量被钳到 20px、陈旧读取算出 48px，都通过了「非 0」的检查。
- **中间态**：写入到布局生效之间有一到两帧，规格明确允许该中间态存在（"任何测量中间态都不得作为最终值长期留在界面上"），但单次读取把它当成最终值。

因此 `e2e/a9-lyrics-offset.mjs` 现在只导出**不变量** `lyricsTop ≥ infoBottom + 20 − 1`，并在 `assertLyricsGapSettles` 里「有界等待 → 复查」：

- 等待有界（3s）：正常情况一两帧内达标；塌缩值在任何时限内都无法达标，所以仍是门禁而不是等待。
- 复查：达标的偏移可能被后一趟更盲目的测量改回去，复查抓这一类。
- 失败信息同时给出内联 margin 与信息块 `display` —— 「没写」与「还没生效」是两个不同的故障，不能读成一句话。

同一类失明出现在专注阅读上：`lyrics-focus-open` 是 passive effect 里加上的，规则绘制晚一两帧，单次读取会把 152px 的底部预留读成 48px。`e2e/a9-lyrics-focus.mjs` 因此先等「信息块 `display: none` 且底部预留 ≥ 100px」这一**绘制后**状态，再断言布局；退出专注后（信息块从无盒回到有盒，塌缩高发点）再次走 `assertLyricsGapSettles`。两个门禁模块同时把 run-e2e.mjs 从 1003 行压回 936 行，守住规模门禁。

## 单元层：把「布局滞后」建进 stub

`useLyricsStartOffset.test.tsx` 的几何 stub 原来让 `rect.top = areaTop + 当前内联 margin`（布局永远即时）。新增 `staleMargin` 选项：指定「布局仍反映的 margin」，模拟旧偏移尚未被替换。判别用例：

- 先按信息块底部 169 测出 89px；
- 让信息块长到 217，同时歌词盒仍回答 `areaTop + 89 = 189`；
- 旧实现写 `217 − 189 + 20 = 48px`，新实现写 `117 + 20 = 137px`。

几何本身在 jsdom 里仍不可成测，真实布局由 A9 的浏览器门禁覆盖。

## 影响边界

只改桌面前端测量 hook、其单测与 e2e 门禁；不改生成的 `player.css`/`responsive.css`、不改窄屏与专注阅读行为、不改 IPC 与 Core。规格只改 IL-R10 的测量条款并追加一个场景，与待定 change `align-immersive-player-and-playlist-directory` 的 delta 无文本重叠（其 IL delta 是布局/主题的静态要求，且其提案文本里对 IL-R10 编号的引用已被 #75 的归档超前，归档时按主规格顺序顺延）。
