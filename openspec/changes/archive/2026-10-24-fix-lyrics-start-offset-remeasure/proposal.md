# Proposal

## Why

宽屏沉浸布局（≥981px）把 `.now-playing-info` 与 `.lyrics` 放进同一 grid area，两者只靠运行时测量的 `--lyrics-start-offset` 分开；该偏移目前只在 `infoSignature`（标题|歌手|专辑|时长）变化或 `window.resize` 时测量一次（`useLyricsStartOffset` 的 `useLayoutEffect`）。自动切歌使沉浸表面按 `currentQueueEntryId` 整体重挂载，凡是「不改变这四个值、却改变信息块高度」的情况——`song_detail` 迟到、CJK 字体回退/加载完成导致行高与折行变化、元信息行折行——都不会触发重算，歌词便画进「歌曲名 / 歌手 / 专辑」区域（issue #75，Windows 必现、macOS 未复现，与两端 WebView 字体度量与 IPC 时序差异吻合）。首曲正常是因为打开表面的那次测量恰好落在最终排版之后。

现有门禁只在「打开沉浸模式后」断言一次几何（e2e A9），切歌路径在 CI 永远是绿的。

## What Changes

- `useLyricsStartOffset` 从「按签名一次性测量」改为「持续观测」：
  - 对 `.now-playing-info` 挂 `ResizeObserver`——高度是唯一事实来源，不关心高度变化的成因（切歌重挂载、明细迟到、字体加载、折行、缩放）；
  - 初次测量保持在 layout effect（避免回归现有打开路径），并在双 `requestAnimationFrame` 后补测一次，避开回退字体下的中间态；
  - `document.fonts.ready` 后补测一次（WebView2 的字体时序是本次 Windows 必现的主因）；
  - 保留既有 `window.resize` 监听与「先移除属性再测量」的方式；
  - jsdom 无 `ResizeObserver`/`matchMedia` 时逐项降级为不适用（既有 `isWideLayout` 语义不变）。
- e2e 门禁 A9 扩展切歌路径：mock 的 `player_control next` 改为真实推进到队列下一首并重发快照（当前只是 `position + 10` 的占位语义，没有任何现有断言依赖它）；A9 在打开断言之后增加「点击下一首 → 重挂载后再次断言 `lyricsTop ≥ infoBottom`」，并注入一次与签名无关的信息块高度变化断言偏移随之重算（判别力：旧实现下该步必红）。
- 单元层新增 `useLyricsStartOffset` 的持续对齐用例（stub `matchMedia`/`ResizeObserver`/`getBoundingClientRect`）：签名变化重装观测、RO 回调按新高度重算、resize 重算。
- 规格：`immersive-lyrics` 新增 requirement「沉浸式歌词起点必须随歌曲信息块几何持续对齐」，登记 IL-R10-S01–S03。

非目标：不动待定 change `align-immersive-player-and-playlist-directory` 的 delta（其 IL 布局 requirement 在该 change 归档时自行回主规格）；不改窄屏（≤980px）堆叠布局；不改 `.now-playing-info` 的渲染内容（截图中的「作曲/编曲」两行在主线无渲染来源，本 change 不为其建规格）；不给组件加 `key` 或改重挂载策略。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `immersive-lyrics`: 新增 requirement——宽屏沉浸布局中歌词起点必须以信息块**实际渲染几何**为唯一事实来源持续对齐：初次测量后信息块高度发生任何变化都必须重算偏移，歌词首行始终位于信息块底部之下并保留固定间距。

## Impact

- **桌面前端（Presentation）**：`apps/desktop/src/features/player/useLyricsStartOffset.ts`（测量机制重写，对外签名不变）；新增 `useLyricsStartOffset.test.tsx`。
- **e2e（Presentation 门禁）**：`apps/desktop/e2e/mock-bridge.ts`（`player_control next` 真实切歌、快照补齐 `currentArtist`/`currentAlbum`）、`apps/desktop/e2e/run-e2e.mjs`（A9 切歌与高度变化断言）。
- **规格与可追溯性**：`immersive-lyrics` delta（ADDED requirement，归档时为主规格第 10 个 requirement → IL-R10-S01–S03）；`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`docs/acceptance/PRD-matrix.md`（A9 路径）、manifest 重生成。
- 不触及 Rust Core、IPC 契约与播放逻辑。
