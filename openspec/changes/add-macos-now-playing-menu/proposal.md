## Why

用户期望 Echo 的歌曲信息出现在 macOS 控制中心的“正在播放”模块，并能从系统媒体控制执行传输操作。现有自绘菜单栏传输控件不发布系统级 Now Playing 信息，因而不能满足这一集成方式。

## What Changes

- 在 macOS 通过系统 Now Playing 服务发布当前歌曲的标题、艺人、专辑、封面、时长、当前进度和播放状态。
- 注册系统远程媒体命令，将播放、暂停、上一首和下一首可靠地转接到 Echo 唯一桌面播放会话。
- 在切歌、暂停/恢复、seek、自然结束、会话恢复和退出时更新或清除系统 Now Playing 状态，防止控制中心显示陈旧内容。
- 保留现有自绘菜单栏传输控件和 Windows/Linux 托盘行为作为独立后台入口；系统控制中心的展示布局及是否成为当前媒体来源由 macOS 决定。

## Capabilities

### New Capabilities

- `macos-now-playing-menu`: 向 macOS 系统 Now Playing/控制中心发布 Echo 的播放信息，并处理系统远程传输命令。

### Modified Capabilities

- `desktop-app-shell`: 明确既有 macOS 菜单栏传输控件与系统 Now Playing 集成并存，且不互相替代。
- `desktop-playback`: 要求唯一桌面播放会话把状态和系统远程命令与 macOS Now Playing 服务保持一致。

## Impact

- 影响 `apps/desktop/src-tauri` 的 macOS 平台适配和构建链接配置，以及 `crates/echo-desktop` 的播放快照到平台集成的投影。
- 复用现有播放协调器、封面缓存和安全的展示元数据；不会向 `echo-core` 引入 MediaPlayer、Tauri 或 macOS 依赖。
- 新增 macOS 平台框架/绑定依赖需记录用途、锁文件和可复现构建；Windows/Linux 行为保持不变。
