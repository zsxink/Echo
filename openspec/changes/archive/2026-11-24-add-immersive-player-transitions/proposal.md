# Proposal

## Why

Issue #85：沉浸式播放器目前是硬切换——`ImmersivePlayer.tsx` 在 `ui.immersiveOpen` 为假时直接 `return null`，打开瞬间整屏从浅色工作区跳到沉浸面，常驻播放栏同时从普通表面变成主题派生表面，封面取色也立刻改写；收起时反向一次性跳回。歌词专注阅读的进出同样是 `display:none` 加整组布局替换，没有过渡。

这条体验缺口不在功能正确性，而在于产品最“决定性”的视觉动作（展开唱片）缺少连续性。需要在不动播放、焦点、Escape 和歌词同步语义的前提下，为进入、退出以及歌词专注切换补上克制、可降级、可中断的过渡。

## What Changes

- **显式过渡状态机**：沉浸式播放器从“打开即挂载 / 关闭即卸载”改为 `closed → opening → open → closing → closed`。退场期间表面继续渲染以完成动画，但立即退出浮层栈、不可交互，焦点回到展开入口；动画结束后才卸载并移除 body 状态。避免依赖跨平台支持不一的 `transition-behavior: allow-discrete` / `@starting-style`。
- **入场与退场**：入场约 220ms（复用 `--motion-base`），封面从常驻播放栏「当前播放区」的封面位置与尺度以 FLIP 方式生长到唱片位置，表面同时淡入并轻微位移/缩放；退场约 180ms（新增 `--motion-exit`），反向收回当前播放区，避免“关门拖沓”。只动画 `transform` 与 `opacity`，不逐帧改模糊半径或歌词高度。
- **颜色与材质连续**：常驻播放栏的玻璃/主题派生色、`--player-tint` / `--player-background` 等已注册颜色变量与表面动画使用同一节奏；封面取色必须保留到退场结束，不能在关闭瞬间先退回主题色。歌词专注切换采用封面/元信息淡化 + 歌词区域连续过渡（不做会引发布局抖动的字号/高度逐帧动画）。
- **切歌连续性与可中断**：移除“按队列条目重挂载整个表面”的做法，切歌只更新封面、元信息、取色与歌词，不重播整面入场；过渡中途反向操作（展开中收起、收起中再展开、切歌）从当前视觉状态继续并收敛到唯一最终状态，不残留 body 类或计时器。
- **减少动画**：`prefers-reduced-motion: reduce` 下所有显隐与专注切换即时完成，无位移、缩放、交叉淡入或延迟卸载；播放、焦点、Escape 顺序与唱片旋转例外保持不变。
- **视觉源与规格同步**：先改 `docs/prototype/echo-desktop-player.html`（CSS 与显隐状态机），再用 `node scripts/extract-prototype-css.mjs` 重生成 `apps/desktop/src/styles/*.css`；`apps/desktop/src/styles/artwork.css`（应用自有浅色沉浸面覆盖层）补充颜色过渡。`immersive-lyrics` 增加一条显隐/专注过渡要求，并登记新的场景与追踪。

非目标：

- 不改变播放、队列、歌词来源、同步、seek、专注模式的既有行为与层级。
- 不引入新的 UI 框架、动画库、Web Animations API 运行时依赖或前端路由。
- 不做液体形变、色散、逐帧模糊、视差或第二套浮层材质。
- 不修改 Rust Core、Tauri command/event、IPC DTO、持久化或平台播放器。
- 不以视觉快照作为唯一验收；自动断言以 DOM 生命周期、计算样式与真实浏览器布局为准。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `immersive-lyrics`: 新增「沉浸式播放器显隐与歌词专注切换必须连续且可降级」要求，覆盖入场来源、退场延迟卸载、专注切换、切歌/中断收敛和减少动画。

## Impact

- 前端：`apps/desktop/src/features/player/ImmersivePlayer.tsx`、`ImmersivePlayer.test.tsx`、新增过渡 hook/测试、`apps/desktop/src/styles/artwork.css`、`apps/desktop/e2e/` 的真实 Chromium 断言。
- 视觉源：`docs/prototype/echo-desktop-player.html`；生成物：`apps/desktop/src/styles/{tokens,player,responsive}.css`（只通过抽取脚本更新）。
- 规格与追踪：`openspec/specs/immersive-lyrics/spec.md` 在本 change 的 delta 中新增要求；实现与归档阶段更新 `docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`。当前基线为 153 个 Requirement / 630 个 Scenario，本 change 预计新增 1 个 Requirement / 5 个 Scenario。
- 文档：`docs/prototype/brand-spec.md` 的动效 token 说明、`docs/interface-terminology.md` 的「当前播放区」/「沉浸式播放器」过渡来源说明（如需）。
- 不影响 Swift/Rust/原生窗口、数据库、同步协议或发布物结构；无迁移与兼容性风险，回滚为单个前端提交。
