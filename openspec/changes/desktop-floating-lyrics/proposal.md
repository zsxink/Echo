## Why

用户聆听本地音乐时常把小窗口切到其他工作之上（写代码、看文档、刷网页），此时需要持续可见的歌词跟随播放进度高亮，而无需停留在 Echo 主窗口或展开沉浸式播放器。为每个步骤只保留基础播放控制于主窗口/托盘的本地优先播放器，提供一个**始终置顶的悬浮歌词窗**是补齐"专注阅读与多任务并行"体验的缺失环节。

## What Changes

- 新增**常驻悬浮歌词窗**：独立无边框小窗，默认显示于主窗口位置附近，可拖拽移动与二段式关闭/隐藏。
- 悬浮窗**始终置顶**（always-on-top），跟随当前播放歌曲、位置与歌词来源；带时间戳歌词按播放进度高亮当前行，点击歌词行 seek 到对应时间。
- 悬浮窗独立渲染**当前歌曲信息与歌词**：库内歌曲经 `get_lyrics` 拉取；临时播放项复用快照中的临时歌词。空态与"仅纯文本"沿用现有歌词语义。
- **完整外观定制**存入本机偏好：字号、透明度、文字/高亮/背景颜色、背景模糊、是否锁定位置、是否始终置顶，全部可选、可安全回退（沿用 `DesktopStateStore` 原子写入 + 默认值回退模式）。
- 设置页新增"歌词悬浮窗"分区，提供开关与外观调整。
- **平台适配**：三平台提供同等能力；macOS 独立窗口置顶层级、Windows/Linux 托盘后台关闭行为以平台差异呈现（三平台全部交付）。

## Capabilities

### New Capabilities

- `desktop-floating-lyrics`: 定义桌面常驻悬浮歌词窗的展示、跟随、交互、外观定制与三平台行为。

### Modified Capabilities

无（既有 `immersive-lyrics`、`desktop-app-shell`、`desktop-playback` 的需求语义不变；悬浮窗是新增能力，不在既有需求的取值域内）。

## Impact

- 新增桌面原生窗口 `lyrics` 与对应 capability/权限集；主窗口 capability 与 CSP 不被扩大。
- 播放快照事件 `player://snapshot` 已携带 `currentSongId`、`position` 与临时歌词，悬浮窗复用同一广播，不新增高频后端事件。
- React 新增悬浮窗组件与外观偏好 hook；Rust 侧新增本地偏好字段与窗口生命周期命令。
- 影响面：`apps/desktop/src-tauri`（窗口装配/能力）、`apps/desktop/src/features/lyrics-window`（新）、`apps/desktop/src/features/settings`（设置分区）、`crates/echo-desktop`（本地偏好）。不进入 `echo-core` 领域层；不新增网络或播放器依赖。
- `docs/interface-terminology.md` 需登记"歌词悬浮窗"术语。