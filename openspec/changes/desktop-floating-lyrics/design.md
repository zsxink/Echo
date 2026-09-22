# Desktop Floating Lyrics — Design

## Context

See proposal.md — Why. 现状（已核实）：Echo 采用 Tauri 2 单窗口 `main`；播放状态经 `player://snapshot` 广播事件以 `UiPlayerSnapshot`（含 `currentSongId`、`position`、`state`、临时项 `currentLyrics`）推送给前端；库内歌曲歌词经 `get_lyrics(songId)` 命令获取（`useLyrics`），来源优先级在 Core 内实现。本机偏好由 `DesktopStateStore` 以原子写 + 默认值回退管理（theme / close_behavior / window）。本期为一期桌面三平台体验的补充能力。

## Goals / Non-Goals

- **Goals**
  - 一个始终置顶、可拖拽、可定制外观的全新歌词窗，复用现有播放快照与歌词解析，不触碰 Core 领域层。
  - 外观与开启状态本机持久化，遵循既有 `DesktopStateStore` 模式。
  - 三平台同等交付（macOS / Windows / Linux），平台差异仅限系统原生行为。
- **Non-Goals**
  - 不新增网络、账号、流媒体或歌词源；不引入新歌词解析规则（覆盖层 > 内嵌 > `.lrc` 语义不变）。
  - 不在悬浮窗内放置播放/暂停/上一首/下一首控件（标准交互经主窗口/托盘）。
  - 不做"系统级全局歌词"（如系统操作系统词飘屏）；本能力限定在 Echo 自有窗口。
  - 不改变现有 Escape 浮层栈与主窗口 capability 边界。

## Decisions

### 1. 独立 Tauri 无边框窗口 `lyrics`，而非主窗口内浮层

新窗口 `lyrics`：`decorations: false`、`alwaysOnTop: true`（默认）、`skipTaskbar: true`、可 `transparent`（开启背景模糊/透明度定制时），由 Rust 装配层按本地偏好创建/更新，前端 Vite SPA 以路由片段或查询参数渲染歌词窗面板。

**备选**：主窗口内绝对定位浮层 — 无法真正"置于其他应用窗口之上"，且主窗口隐藏（后台运行）时不可见，不满足需求，排除。**备选**：Windows/Linux 使用 `WebviewWindowBuilder` + 不脱离主进程的原生 layered window — 复杂度高且跨平台不可移植，排除。

### 2. 数据流复用 `player://snapshot` 广播，库内歌词走 `get_lyrics`

两种播放形态：
- 库内歌曲：快照给出 `currentSongId`，歌词窗按"当前条目/歌词来源变更 → `get_lyrics`"拉取，与 `useLyrics` 同构（各自组件内 hook，不做全局注入）。请求失败降级为空态，路径规则与现有 `useLyrics` 一致。
- 临时播放项：快照已携带 `currentLyrics`（`UiTemporaryLyrics`），歌词窗直接采用，无需额外后端调用。

**理由**：`UiPlayerSnapshot` 已是受控广播，窗口内订阅不新增事件；不把歌词拉取挤进快照广播（快照 10 Hz，歌词低频）。**备选**：把库内歌词也塞进 `UiPlayerSnapshot.current_lyrics` — 每 10 Hz 重建歌词列表，高频且改动快照契约，排除。

### 3. 歌词行高亮/seek 复用 `findCurrentLine` 二分逻辑

歌词窗内实现与 `LyricsColumn.tsx` 相同的当前行二分查找与"点击行 → `seek({seconds})`"；播放/暂停状态由快照 `state` 保持，seek 后依赖快照回传校正位置，不本地伪造。

**理由**：符合"快照为唯一权威"约束（CODE_STANDARDS §3、`playerStore` 注释）。**备选**：抽公共歌词列组件 — 布局与容差（沉浸式居中、跟随窗可滚动）差异大，强行抽象收益低，仅在拆出后确有重复时再聚合。

### 4. 外观与位置存入 `DesktopStateStore`，沿用原子写入 + 默认回退

在既有 `local_state` 文档追加 `lyricsWindow: { enabled, fontSize, opacity, textColor, highlightColor, backgroundEnabled, blurEnabled, locked, alwaysOnTop, position?, themeSurface? }`；`ResolvedState` 同步扩展并提供 `Safe` 默认。所有 setter 走现有 `mutate`（原子写文件、失败保留旧文件）；未知/非法枚举值按现有 `to_resolved` 回退逻辑落到安全默认。

**保持** 现有回退契约（corruption → defaults，不阻塞启动）；新增字段与旧文档向后兼容（缺省字段按默认值加载），完好存量文档不因升级重写。

### 5. 三平台差异最小化，仅限原生窗口行为

- **macOS**：`alwaysOnTop` 依赖 Tauri 平台实现（NSFloatingWindowLevel）；窗口随关闭/开启跟随本机偏好。
- **Windows/Linux**：`alwaysOnTop` + `skipTaskbar`；后台运行时窗口关闭按"仅隐藏"还是"销毁"由偏好决定（与主窗口 `CloseBehavior` 语义对齐：exit 时随进程销毁，background 时保留或按用户开关隐藏）。
- 窗口不注册为 `file-association`、不参与单实例 argv、不参与封面 `cover://` 之外的新协议。

### 6. 前端窗口标识与装配

Vite 侧以单页路由参数区分 `main` 与 `lyrics` 两个挂载点（同 bundle），避免第二个 entry/重复 CSP 外资源；Rust 侧只在本地偏好 `enabled` 时于 `setup` 装配 `lyrics` 窗口，全生命周期由 `desktop-app-shell` 侧扩展管理（显示/隐藏/销毁/位置保存）。`cover://`、`get_lyrics`、`seek`、`player://snapshot` 订阅为 `lyrics` 窗口 capability 白名单的最小集。

## Risks / Trade-offs

- **[透明/模糊跨平台渲染差异]** → 透明由各平台 WebView 落下为准；样式以"不透明后台兜底"设计，模糊不可用时退化为不透明底色。
- **[`seek` 后位置回传抖动]** → 不本地平滑，仅依据快照校正；与现有播放栏行为一致。
- **[窗口与主窗口 Escape/焦点栈解耦]** → 歌词窗是独立顶层窗口，不参与主窗口 overlay 栈；以自身显式脚本/ESC 关闭，规避"多窗口共用焦点模型"。
- **[偏好文档新增字段的向后兼容]** → 旧文档缺字段按默认加载；serde 缺省而非报错，且 `to_resolved` 已有安全回退路径。
- **[alwaysOnTop 的平台实现差异]** → 以最小平台代码封装窗口设置；不引入额外原生依赖。

## Migration Plan

- 纯新增能力；不迁移数据、不改既有 IPC 契约（快照与命令不变）。
- 回滚：移除 `lyrics` 窗口装配代码与偏好字段即可，不影响 play / library / settings 既有路径。
- 本地偏好文件保持向后兼容（新增字段可选）。

## Open Questions

- 歌词窗的默认初始位置（主窗口右上角 vs. 上次位置）：已定为"保存位置优先，首次开启以主窗口右上角为默认"，不阻塞实现。
- 是否在托盘菜单增加"歌词悬浮窗"显隐入口：本期以设置开关为主，托盘入口属可有可无的增强（可作为后续小 change，不改变本设计）。