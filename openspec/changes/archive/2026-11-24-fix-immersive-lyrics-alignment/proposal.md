# Proposal

## Why

Issue #168：#85 为沉浸式播放器加入入场过渡后，从常驻播放栏展开播放器时，歌词列不再定位到当前播放行。根因不是歌词数据或阶段机，而是 `LyricsColumn` 的滚动对齐在入场过渡进行中执行：宽屏布局用「唱片中心」作为滚动目标，但此刻 `.playing-cover` 正被 FLIP 变换到常驻播放栏的当前播放区，`getBoundingClientRect()` 量到的是被 `transform` 扭曲后的矩形。错误几何还会写入 `--lyrics-edge-top/bottom`；由于 `ResizeObserver` 只看布局尺寸、`transform` 不改变布局尺寸，过渡结束后不会自动纠正。暂停状态下展开时没有后续换行触发，错误位置会一直保持。

这是 #85 交付范围内的回归：过渡本身符合规格，但歌词同步的既有承诺（当前行可见、自动滚动跟随）在入场路径上被破坏。

## What Changes

- **入场期间不写滚动**：把沉浸表面阶段（`opening` / `open` / `closing`）传给歌词列；`opening` / `closing` 期间跳过对齐，不使用被入场 `transform` 扭曲的矩形写入 `scrollTop` 或 `--lyrics-edge-*`。
- **稳定几何后对齐一次**：表面与封面的 `transform` 过渡结束后（以 `transitionend` 为主、`--motion-base` 时长为兜底），按稳定几何执行一次对齐；宽屏目标仍为唱片中心，窄屏与歌词专注阅读仍为歌词视口中心。
- **覆盖晚到与重建路径**：歌词在入场结束后才到达、切歌、收起后重新展开、从歌词专注阅读返回时，按当前阶段决定立即对齐或等待稳定几何；暂停状态下展开同样定位到当前行。
- **减少动画**：`prefers-reduced-motion: reduce` 下不等待过渡，立即对齐。
- **回归验证**：组件测试断言入场期间不写滚动、稳定后写入正确落点；真实 Chromium 断言展开 settle 后当前行位于歌词视口内并接近唱片中心，并覆盖暂停展开。

非目标：

- 不改变入场/退场的视觉设计、时长、FLIP 几何或阶段机语义。
- 不改变歌词来源优先级、当前行判定、seek、手动滚动暂停 5 秒、歌词专注阅读入口或 Escape 分层。
- 不引入新的动画库、布局库或跨层依赖；不修改 Rust Core、Tauri command/event、IPC DTO、持久化或平台播放器。
- 不重写 `useLyricsStartOffset` 的信息块起点测量；本次只处理「当前行滚动落点」。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `immersive-lyrics`: 新增「沉浸式播放器展开后的歌词定位必须基于稳定几何」要求，覆盖入场期间禁止写入、过渡结束后对齐、暂停展开、晚到歌词、切歌/重新展开与减少动画。

## Impact

- 前端：`apps/desktop/src/features/player/LyricsColumn.tsx`、`ImmersivePlayer.tsx` 及对应组件测试；必要时在 `useImmersiveTransition.ts` 暴露可复用的稳定时机判定。
- 真实浏览器验收：`apps/desktop/e2e/a85-immersive-transition.mjs`（或 A9 系列）补充「展开后当前行在视口内且接近唱片中心」与「暂停展开」断言。
- 规格与追踪：`openspec/specs/immersive-lyrics/spec.md` 通过本 change 的 delta 新增 1 个 Requirement / 5 个 Scenario；实现与归档阶段同步 `scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`、`docs/traceability.md`，保持三方场景集合一致。
- 不影响播放状态、队列、歌词来源、IPC、数据库、同步协议或发布物结构；无迁移与兼容性风险，回滚为单个前端提交。
