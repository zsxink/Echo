# Proposal

## Why

带刘海屏的 MacBook 在屏幕顶部正中有一块物理遮挡区，菜单栏被切成左右两段，用户在其它应用里想暂停或切歌时只能移开视线或切回 Echo 主窗口。刘海两侧的空白区域是这类机型上唯一常驻可见、且不占用任何窗口的界面资源，把它做成 Echo 的第三个播放入口可以让播放控制始终在手边。

本次变更同时解决两件事：把已经过视觉验收的原型（`docs/prototype/echo-macos-notch-player.html`）落地为真实窗口；并让「是否显示」成为用户可控、且在没有刘海屏的 Mac 上明确不可用的本机偏好。

## What Changes

- **新增 macOS 刘海播放器窗口**：与主窗口同进程的伴随窗口，贴在带刘海屏幕的顶边正中，收缩态只比硬件刘海宽左右各 26 px（左小封面、右跳动音符），鼠标悬浮时向右拓宽并向下展开为完整播放控制（封面、歌名、歌手、跳动音符、可拖动进度条、喜欢 / 上一首 / 播放·暂停 / 下一首 / 播放模式）。
- **只在带刘海屏的 Mac 上存在**：启动与屏幕参数变化时检测刘海辅助区域；没有刘海的 Mac、外接显示器、Windows 与 Linux 上不创建该窗口，也不保留任何可见痕迹。
- **新增本机偏好「刘海播放器」**：位于设置「应用行为」分组，macOS 专属。有刘海屏时开关可用且默认开启；没有刘海屏时开关置灰不可选，并说明原因。偏好持久化在本机 desktop-state，损坏或不可写时安全回退到默认值。
- **播放模式不使用主题强调色**：刘海播放器的播放模式控件在列表循环、随机播放、单曲循环三种状态下都使用与上一首 / 下一首一致的中性色，只靠图标区分，与 `desktop-playback` 对常驻播放栏随机模式的既有要求保持一致。
- **规格澄清**：`desktop-app-shell` 的「单实例」要求明确区分「重复启动产生的第二个实例」与「同一进程内的伴随窗口」，并为新增偏好补一条设置项要求。
- 复用既有播放权威通道：不新增播放器、队列或进程；控制走既有 `player_control` / `seek` / 收藏通道，状态来自既有 `player://snapshot` 事件。
- 界面事实来源：新增的独立原型 `docs/prototype/echo-macos-notch-player.html` 的产品样式段纳入 `scripts/extract-prototype-css.mjs` 生成流程，产物为 `apps/desktop/src/styles/notch.css`（禁止手改）。

## Capabilities

### New Capabilities

- `macos-notch-player`: macOS 刘海播放器窗口的存在条件、几何与层级、收缩与展开两态的可见内容、播放状态与控制的同源性、与其它播放入口的一致性、无刘海环境下的不出现，以及「刘海播放器」本机偏好的默认值与不可用状态。

### Modified Capabilities

- `desktop-app-shell`: 单实例要求需区分「第二个实例」与「同进程伴随窗口」；设置要求需覆盖新增的「刘海播放器」开关及其「无刘海时置灰」的不可用状态。

## Impact

- **桌面壳（Rust）**：新增刘海几何检测与伴随窗口管理（AppKit FFI，新增 `NSScreen` 相关 `objc2-app-kit` feature）；`main.rs` 装配与生命周期；新增两条 IPC command（读取 / 写入「刘海播放器」偏好及其可用性）。
- **本机偏好**：`crates/echo-desktop/src/platform/local_state.rs` 增加 `show_notch_player` 字段，沿用逐字段宽容解析与原子写入。
- **前端**：新增伴随窗口入口与视图（复用 `playerStore`、`useCoverKeys`、`useSongDetail`、`PlayerProgress` 与既有传输控件样式）；`SettingsView` 增加 macOS 专属开关行；`bridge` 增加两条命令类型；生成物 `ipc-types.generated.ts` 由 `pnpm --dir apps/desktop generate:ipc` 重生成。
- **样式**：新增 `apps/desktop/src/styles/notch.css`（由原型生成）；`scripts/extract-prototype-css.mjs` 增加第二个提取目标。
- **权限面**：新增 `apps/desktop/src-tauri/capabilities/notch.json`，只给伴随窗口最小权限；不引入 `fs` / `shell` / `dialog` / `process` 权限族。
- **文档与规格**：`docs/PRODUCT.md` 的桌面运行上下文、`docs/interface-terminology.md`（新增「刘海播放器」术语）、`docs/ROADMAP.md`、`openspec/specs/desktop-app-shell` 与新增 `openspec/specs/macos-notch-player`。
- **不做**：不改播放队列、播放模式语义、播放统计、会话恢复；不实现 Windows / Linux 版本；不把刘海播放器做成键盘可聚焦的窗口（非激活窗口无法获得键盘焦点），因此它不得成为任何播放功能的唯一入口。
