## Context

Echo 当前拥有一个桌面播放协调器、Tauri/AppKit 菜单栏适配器、播放器快照和不透明封面缓存。现有菜单栏适配器是 Echo 自绘的直接控制入口，并不是 macOS 控制中心的 Now Playing 集成。

macOS 通过 MediaPlayer 的 `MPNowPlayingInfoCenter` 接收媒体元数据与播放状态，并通过 `MPRemoteCommandCenter` 提供系统媒体命令。macOS 决定是否及如何展示 Now Playing 卡片；Echo 只提供准确的元数据和命令处理。

## Goals / Non-Goals

**Goals:**

- 将唯一桌面播放会话投影到 `MPNowPlayingInfoCenter`，且不泄漏文件路径。
- 将系统播放、暂停、上一首和下一首命令一次性路由到既有协调器，并如实报告结果。
- 在切歌、进度、状态、恢复和退出时保持系统信息准确。
- 隔离所有 macOS 框架调用于 Tauri 平台层，保持 `echo-core` 跨平台。

**Non-Goals:**

- 绘制、定位或强制显示 macOS 控制中心的 Now Playing 卡片。
- 替换现有菜单栏控制、媒体键支持、队列语义或 Windows/Linux 托盘。
- 新增播放会话、远程流媒体或持久化资料库数据。

## Decisions

### 1. 在平台层增加 macOS MediaPlayer 适配器

在 `apps/desktop/src-tauri` 新增 macOS-only 适配器，负责框架初始化、Now Playing 发布、远程命令注册和退出清理。通过受维护的 Rust binding 或最小的 Objective-C bridge 链接 Apple MediaPlayer 框架，并记录依赖用途。MediaPlayer 类型不得跨入 `echo-desktop` 或 `echo-core`。

替代方案是扩展自绘 `NSStatusItem` 显示标题与封面；它不能满足控制中心的系统级 Now Playing 目标，因此不采用。

### 2. 从现有权威状态构建安全投影

组合根订阅播放器端口并通过协调器获得当前队列项。平台投影经既有元数据和封面服务解析标题、艺人、专辑、时长、进度、状态和不透明 artwork，不包含绝对路径。临时项仅发布已知展示字段；缺失可选字段直接省略。

在切歌和状态改变时立即发布；位置事件做限流并归一化到时长范围。异步封面结果携带 entry identity/revision，丢弃过期结果，避免上一首封面覆盖当前歌曲。

不使用 WebView 的 `player://snapshot` 或 `cover://`，因为主窗口隐藏后系统集成仍必须工作，且原生层不应依赖渲染器生命周期或 CORS。

### 3. 一次注册系统命令并复用既有命令通路

适配器在应用生命周期仅注册一次播放、暂停、上一首和下一首处理器。每个处理器委托既有 `RuntimeStatusMenuSink`/协调器命令路径，并且仅在操作被会话接受时向系统返回成功。系统控制与已有状态栏按钮同为同一粗粒度命令来源；队列语义仍只属于 `PlaybackCoordinator`。

直接控制 mpv 被拒绝，因为它会绕过队列、自动推进和会话权威。

### 4. 明确清理系统状态并保留状态栏入口

空/停止会话和显式退出时清除 Echo 发布的 Now Playing 状态。恢复会话时发布暂停的恢复项和有效进度，但绝不自动播放。既有 `NSStatusItem` 保持为独立后台入口，不试图渲染控制中心内容。

## Risks / Trade-offs

- [系统拥有卡片可见性与布局] → 以元数据发布和命令往返为验收，不以固定截图作为保证。
- [MediaPlayer 绑定/桥接增加构建风险] → 仅在 macOS cfg 下链接，隔离模块并在 macOS CI 构建。
- [重复注册回调] → 仅应用生命周期注册，并测试每个请求最多向协调器发一次命令。
- [元数据或封面解析竞争] → 以 entry identity/revision 拒绝陈旧结果。
- [高频进度更新] → 合并、限流进度更新，而对切歌和状态变化立即发布。

## Migration Plan

1. 增加 macOS-only 适配器，并以 cleared/idle 状态初始化。
2. 在播放器组合完成后接入快照、元数据和封面服务，并注册远程命令。
3. 在 macOS 验证系统选择展示 Echo 时，控制中心信息正确且按钮操作回到同一播放会话。
4. 适配器或 artwork 失败时不影响正常播放和既有状态栏入口，清除或省略不可用系统信息并记录无路径诊断。
5. 回滚仅删除适配器和 macOS 框架链接，无数据迁移。
