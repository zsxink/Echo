# Echo 视觉基准（Design Language）

> 来源：珊瑚玫红图标 `c912806cc463266fde4e9de00ca23ddd.jpg` 与深钴蓝参考 `47d7bd80bc0e716c4a2a463aa7a88ced.jpg`。
> 权威实现：`docs/prototype/echo-desktop-player.html` 的 `:root` 与三主题块。本文档是设计语言的可读基准，色值以 OKLch 作为实现基准；实现若与原型 CSS 有出入，以原型为准。
> 默认主题为珊瑚玫红（wine），提供深钴蓝（cobalt）与松石绿（green）两种可选主题。

## 1. 设计语言总述

Echo 是本地优先、自用优先的音乐播放器。视觉基调是**克制、安静的应用工作区**：近白背景 + 纯白表面承载高密度数据控件，单一主题色承担音乐符号、主要动作与选中/焦点信号。对照 Ant 企业级数据密集风格，但弱化"面板感"，突出纯白与留白。整体要点：

- **纯白工作表面**：背景与卡片均为近白/纯白，用细分隔线（`--border-soft`）与留白分层，而非阴影堆叠。
- **单一强调色、每屏至多两处**：主题色用于音乐符号 + 主要操作。收藏心形独立使用红色语义，与主题色无关。
- **紧凑数据控件**：表格行高、导航项、工具按钮按小步长（4/8/12px）布局，服务曲库的信息密度。
- **一个决定性视觉动作**：沉浸式播放器的动态黑胶唱片 + 唱臂是唯一的大型装饰性表达；常规工作区保持朴素。

## 2. 色板 Token

以下 token 以原型 CSS 为准，采用 OKLch 书写。`--bg` 到 `--danger` 为核心工作区 token；`--accent-strong` 用于按钮实底与选中强调。

```css
:root {
  --bg: oklch(1 0 89.88);
  --surface: oklch(0.9642 0 89.88);
  --surface-warm: oklch(0.9460 0.0160 19.38);
  --fg: oklch(0.2134 0 89.88);
  --fg-2: oklch(0.4180 0.0250 19.38);
  --muted: oklch(0.5100 0.0160 19.38);
  --accent-strong: oklch(0.53 0.2219 19.38);
  --accent-strong-hover: color-mix(in oklab, var(--accent-strong), black 8%);
  --theme-cobalt: oklch(0.3787 0.1953 263.35);
  --meta: var(--theme-cobalt);
  --border: oklch(0.8940 0.0090 19.38);
  --border-soft: oklch(0.9300 0.0050 19.38);
  --accent: oklch(0.6528 0.2219 19.38);
  --accent-on: oklch(1 0 89.88);
  --accent-hover: color-mix(in oklab, var(--accent), black 8%);
  --accent-active: color-mix(in oklab, var(--accent), black 14%);
  --success: #22a06b;
  --warn: #faad14;
  --danger: #cf1322;
}
```

> 实现注记：
> - `--muted` 以实际实现（较取样略加深至 0.51 L）为准，保证正文/弱化文字在白底 ≥4.5:1。
> - `--accent-strong`（0.53 L）比 `--accent`（0.653 L）更深，用于按钮实底与选中文字，确保白字对比；`--accent` 用于滑块、进度、图标与边框高亮。
> - `--surface-warm` 是"选中背景"，用当前主题色去饱和的浅色底（见主题块），仅在选中/进行中的导航项、工具与主播放按钮使用。
> - `--danger` 只服务于"收藏心形（已收藏实心/取消收藏的悬停）"与破坏性操作（清空队列、删除歌曲、危险菜单项）。
> - 状态色沿用 Ant：`--success`（在线状态点）、`--warn`（封面占位分级）、`--danger`。

### 2.1 三主题

主题仅改变强调色（`--accent`、`--accent-strong`、`--surface-warm`）与唱片视觉的取色；**资料库工作区保持纯白；沉浸模式使用主题派生深色背景**。`--meta`（深钴蓝）在珊瑚玫红与松石绿主题下保持不变，作为次要语义标记（breadcrumb/元信息）。

```css
:root { /* 默认：珊瑚玫红 wine */ }

:root[data-echo-theme="cobalt"] { /* 深钴蓝 · 冷静、专注 */
  --accent: var(--theme-cobalt);
  --accent-strong: oklch(0.3100 0.1850 263.35);
  --accent-strong-hover: color-mix(in oklab, var(--accent-strong), black 8%);
  --surface-warm: oklch(0.9460 0.0140 263.35);
}

:root[data-echo-theme="wine"] { /* 珊瑚玫红 · 默认 */
  --accent: oklch(0.6528 0.2219 19.38);
  --surface-warm: oklch(0.9460 0.0160 19.38);
}

:root[data-echo-theme="green"] { /* 松石绿 · 平衡、舒缓 */
  --accent: oklch(0.54 0.1330 159.56);
  --accent-strong: oklch(0.44 0.1330 159.56);
  --accent-strong-hover: color-mix(in oklab, var(--accent-strong), black 8%);
  --surface-warm: oklch(0.9460 0.0180 159.56);
}
```

主题选择保存在本机偏好（`localStorage`），并在多标签页间同步；跨主题的选中仍保持同一套"浅色底 + 主题色文字"规则。

## 3. 排版

- 显示字体：`"Ant Sans", "Alibaba PuHuiTi", Inter, Arial, sans-serif`
- 正文字体：`"Ant Sans", "Alibaba PuHuiTi", Inter, Arial, sans-serif`
- 等宽字体：`"SF Mono", ui-monospace, Menlo, monospace`
- 标题用显示字体并带 `--tracking-display: -0.018em`；正文行高 `--leading-body: 1.52`，标题行高 `--leading-tight: 1.08`。

字号阶梯（`--text-*`）：`12 / 14 / 16 / 18 / 22 / 32 / 48 / 64px`。

- 数据/计数/时长/导航标签使用等宽字体（`--font-mono`）+ 字母间距（eyebrow `.08em`）。
- 层级：页面标题多用 `--text-xl/2xl`（表格工作区标题保持 `--text-xl` 的克制），标题始终显示字体 + 650–750 字体权重；正文为 `--fg`，次要/元信息为 `--fg-2` / `--muted`。

## 4. 间隔、圆角、阴影

- **间距**：4 / 8 / 12 / 16 / 20 / 24 / 32 / 48px（`--space-1..12`）；分组小步长 2px。
- **圆角**：`--radius-sm 6px`（输入、列表项、小控件）、`--radius-md 10px`（按钮、浮层）、`--radius-lg 16px`（对话框、面板）、`--radius-pill 9999px`（滑块柄、状态点）。
- **阴影**：默认平铺（`--elev-flat`）；浮层/菜单/对话框用 `--elev-ring`（1px 描边）与 `--elev-raised`（`0 18px 42px rgba(31,31,31,.10)`）。优先用描边 + 留白而非重度投影。
- **焦点环**：`--focus-ring: 0 0 0 4px` accent-strong 的 76% 透明；所有可聚焦控件必须有清晰的 `:focus-visible`。

## 5. 动效

- 时长：`--motion-fast 140ms`（悬停/状态确认）、`--motion-base 220ms`（浮层入场/侧边栏）。
- 缓动：`--ease-standard cubic-bezier(0.2, 0, 0, 1)`（前端加载）。
- 唱片旋转 `record-spin 12s linear infinite`；唱臂抬起 `480ms cubic-bezier(.2,.72,.2,1)`（类弹性而非纯曲线）。
- 尊重 `@media (prefers-reduced-motion: reduce)`：将 transition/animation 降为 1ms 且动画只执行一次。

## 6. 图标与音乐符号

- 品牌与音乐符号以**纯白底 + 高对比、完整连贯的八分音符轮廓**为准，需在小尺寸下保持辨识度；随主题色着色（`currentColor` 为 `--accent`）。
- 常规功能图标：1.8px 描边线条图标（`stroke="currentColor"`），18–20px；播放/暂停、上一首下一首、顺序/随机/单曲循环等使用实心填充图标（`fill="currentColor"`）以区分控制语义。
- 收藏心形图标独立使用红色语义：未收藏为空心描边，已收藏为实心（`--danger` 填充）。
- 封面占位：无封面/无图片时用主题色取色的径向占位（细第 8 音符圆盘图形）；`--warn`、`--success`、`--fg-2` 混入黑用于多级占位色板 A–E。

## 7. 沉浸式播放器（决定性视觉动作）

- 宽于 980px：左侧唱片与唱臂，右侧元信息和歌词。元信息顶部对齐唱臂支点；歌词从元信息下方开始，当前整句中心对齐唱片中心。根据真实行高计算首尾留白，长句换行和首尾句仍可对齐。
- 761–980px：唱片、信息和歌词居中纵向排列，取消宽屏偏移。≤760px：信息 → 唱片 → 五行左右歌词预览，歌词独立滚动，底部控制预留安全区；这些是桌面窗口适配，不代表已交付移动原生客户端。
- 最新背景以原型末尾覆盖规则为准：`--player-background` 由主题强调色与深色底混合，播放器、播放栏和队列共用；左侧径向光晕使用 `--player-glow`。不是封面自动取色，也不是半透明播放栏。
- 当前歌词使用 `--accent-on` + 650 权重；普通歌词为白色混入 30% 透明。主题背景颜色过渡为 800ms ease-out，减少动画时取消该过渡。
- 宽屏与窄屏的歌词专注状态隐藏唱片和歌曲信息，保留歌词、收起入口及底部控制。收起先退出专注，再退出播放器；滚动后 180ms 内的区域点击不切换专注。
- 常驻播放栏的播放/暂停采用主题色实心字形，不使用实底圆；桌面热区 44px、图标 30px。展开入口是当前播放区的封面和文字卡片。

## 8. 视觉规则（可检查清单）

1. 近白背景与纯白表面建立克制、安静的应用工作区；数据密集区域用细分隔线分层。
2. 主题色用于音乐符号、主要操作、选中与键盘焦点，每屏至多两处；收藏心形独立红色语义。
3. 选中态使用与当前主题匹配的极浅色背景（`--surface-warm`）+ 主题色文字，不加额外装饰。
4. 图标保持纯白底与高对比、完整连贯的八分音符轮廓并适配小尺寸；功能图标统一 1.8px 描边。
5. 主题仅改变音乐符号与主要操作色，资料库工作区背景纯白，沉浸模式使用主题派生深色背景；选择本机持久化。
6. 深色沉浸面（播放器模式）内，文字/图标用 `--accent-on` 及其淡化层级，保证对比不下滑；`prefers-reduced-motion` 下动画降级。
7. 珊瑚玫红主题从 `c912806cc463266fde4e9de00ca23ddd.jpg` 图标主体采样，`oklch(0.6528 0.2219 19.38)` 为实色实现；`--accent-strong` 加深以便白字按钮对比。

## 9. 最新原型对照边界（2026-09-30）

- 全部歌单采用卡片目录，包含封面、名称、歌曲数、拖动标记与独立编辑按钮；编辑弹窗预填名称，并提供封面选择和预览。
- 主规格中的真实计数、请求失败恢复、歌词来源、歌词行 seek、手动滚动暂停跟随规则仍然有效；固定曲目、50 句测试歌词、定时器模拟导入/扫描不属于真实数据和提交成功证据。
- 原型展示歌单名称/创建时间排序菜单，但 `renderPlaylistDirectory()` 当前直接读取侧边栏顺序，未应用所选字段排序；规格记录目标行为，不能据此宣称排序实现已完成。
- 原型歌词区域点击演示专注切换，未完整实现歌词行 seek 与五秒暂停跟随；实现须隔离区域切换和歌词行定位，不能删除既有业务验收场景。
- `--quality-sq` 与 `--quality-hq` 为音质标签文字色；深钴蓝主题单独覆盖以区分金色 SQ 与蓝色 HQ。具体格式判定以 `library-experience` 为准。
