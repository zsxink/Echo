# Design

## Context

视觉与交互基准已经过用户验收，成品原型在 `docs/prototype/echo-macos-notch-player.html`（独立文件，含一段验收台演示样式，不属于产品界面）。行为契约见本变更的两个 delta spec。

与实现直接相关的现状：

- 主窗口由 `tauri.conf.json` 的 `app.windows` 声明，`capabilities/main.json` 的 `windows` 只有 `["main"]`。
- 播放状态由 `player://snapshot` / `player://queue` / `player://effects` 三条事件下发，`apps/desktop/src/player/playerStore.ts` 是唯一的 `useSyncExternalStore` 边界；控制走既有 `player_control` / `seek` / `set_favorite` 命令。
- macOS 已有两处 AppKit FFI 岛屿可作为范式：`macos_status_row.rs`（`NSStatusItem` 自绘）、`macos_window_lifecycle.rs`（`NSWindow` 通知观察者）。`apps/desktop/src-tauri/Cargo.toml` 的 `objc2-app-kit` 尚未启用 `NSScreen` / `NSVisualEffectView` / `NSWindowCollectionBehavior` 所需 feature。
- 本机偏好集中在 `crates/echo-desktop/src/platform/local_state.rs`：`RawDoc` 逐字段宽容解析，未知/错类型只让该字段回退，`ResolvedState` 记录 `had_corruption`。
- 桌面样式是 `docs/prototype/echo-desktop-player.html` 经 `scripts/extract-prototype-css.mjs` 逐字切分的生成物，禁止手改。

## Goals / Non-Goals

**Goals**

- 一个与主窗口同进程的伴随窗口，状态与控制完全复用唯一播放会话。
- 窗口几何、层级、焦点行为在真机上不干扰用户当前工作。
- 「是否有刘海」与「用户是否开启」是两个独立条件，二者都满足才存在窗口。

**Non-Goals**

- 不做键盘可聚焦的播放入口（非激活窗口无法成为 key window，见 Decisions D6）。
- 不改播放队列、播放模式语义、统计与会话恢复。
- 不为 Windows / Linux 提供等价物。

## Decisions

### D1 用第二个 Tauri WebviewWindow，而不是原生 AppKit 面板

前端已有播放状态、封面查询、进度控件与命令封装，伴随窗口直接复用同一套 React 代码即可；纯原生面板要重写五个控件、跳动音符与状态投影，形成两套 UI 与两套状态模型，也与 #55 的后续重写重复投入。

代价是第二个 WebView 的内存占用。该页面不渲染曲库、列表或歌词，只有一块面板，可接受。

### D2 刘海检测用 `NSScreen.auxiliaryTopLeftArea` / `auxiliaryTopRightArea`

两者都存在且宽度非零即判定有刘海，同时用它们反推刘海矩形（`notchWidth = screenWidth - left.width - right.width`）与两侧可交互区，供窗口水平居中。这两个 API 与项目最低系统版本 macOS 12.0 同时可用。

`NSScreen.safeAreaInsets.top > 0` 只能给出「顶部被遮挡」这一个布尔量，拿不到刘海宽度，不足以把窗口精确对齐，因此不采用。

可用性判定遍历当前 `NSScreen.screens`：任一屏幕有刘海即视为可用，窗口放到那块屏幕上；内建屏合盖后它不在列表里，判定自然变为不可用。

### D3 窗口随状态改变尺寸，而不是固定最大尺寸 + 透明区域穿透

收缩态窗口 238×32、展开态 412×196（面板本体 360 + 左右外翻角各 26）。固定最大尺寸的方案需要在没有悬停事件的情况下动态开关 `setIgnoreCursorEvents`，才能让透明区域不吞掉下方应用的点击，判定复杂且容易误吞。

改变尺寸的代价是每次展开/收起要做一次窗口 resize。为了让动画不被窗口边缘裁剪，**先 resize 再播放 CSS 过渡**：展开时前端 `await` 一次 `notch_player_set_expanded(true)`，Rust 完成 resize 与重新定位后才把 `data-expanded` 置上；收起时先播放过渡，过渡结束后再调用收起。

resize 时必须同时重新定位：Tauri 的 `set_size` 保持左上角，而这里要保持「顶边贴屏幕顶、水平以刘海中心对齐」，所以 Rust 侧按新尺寸重算 x。

### D4 液态玻璃用 `NSVisualEffectView`，不用 CSS `backdrop-filter`

透明窗口里 WebView 的 `backdrop-filter` 只能模糊窗口内部已合成的内容，模糊不到窗口后面的桌面 —— 原型里能看到模糊，是因为原型页把模拟桌面画在了同一个文档里。真实实现若只写 `backdrop-filter`，展开态会是一块没有模糊的深色半透明块。

因此窗口背景挂一层 `NSVisualEffectView`（`blendingMode = .behindWindow`，`material = .hudWindow`），WebView 背景透明；收缩态 CSS 底色不透明（`rgb(11,13,18)`）自然遮住它，展开态 CSS 底色半透明（`rgba(11,13,19,.86)`）让系统模糊透出来。CSS 侧的顶部高光渐变与外投影保持不变。

若真机上 vibrancy 表现不达标，退化方案是把展开态底色提到接近不透明（`.94`），只保留高光与外投影 —— 视觉会接近「哑光黑面板」，但不会退化成一块脏灰。

实现时确认了一条平台前置：Tauri 在 macOS 上要真正得到透明窗口，必须启用 `tauri` 的 `macos-private-api` feature，并在 `tauri.conf.json` 声明 `app.macOSPrivateApi: true`（构建脚本会校验两者一致，缺一会直接构建失败）。这会让 Tauri 打开私有 API 代码路径，只在本窗口请求透明时生效；Echo 通过 DMG 分发，不涉及 App Store 审核约束。不启用时窗口不透明，vibrancy 会被底色完全遮住，「液态玻璃」退化成纯色面板。

### D5 刘海播放器不承接键盘焦点

非激活窗口不能成为 key window，`Tab` 导航与 VoiceOver 遍历在真机上不可靠。因此刘海播放器定位为**指针专用辅助入口**：它不是任何播放功能的唯一入口，同等操作必须仍可通过常驻播放栏、菜单栏状态项与系统媒体键完成（已写入 spec）。控件仍保留 `aria-label`，但不承诺键盘路径。

这条决定否掉了原型里「Tab 可聚焦 / Esc 收起」的演示行为 —— 那是浏览器里的效果，不是窗口的真实能力。

### D5b 整条收缩态都是展开命中区，包括刘海本体

窗口在最上层（`NSStatusWindowLevel`）覆盖了刘海所在的 185 × 32 区域，因此这块区域的指针命中本来就归窗口所有。把命中区限定成两侧窄耳并不能「把刘海还给下层应用」——它只会让用户最难瞄的中间一块变成死区。因此命中区取整条收缩态，包括刘海本体。

代价写清楚：下层应用在这块区域收不到指针进入事件。但该区域被物理刘海遮挡、没有任何可见内容，实际损失为零。

### D6 偏好存本机，IPC 用一对专用命令而不是塞进 bootstrap 快照

`RawDoc` 增加 `show_notch_player: Option<bool>`，`ResolvedState` 增加 `show_notch_player: bool`，默认 `true`；非布尔值只让该字段回退并置 `had_corruption`。

`get_bootstrap_state` 是冷启动的大快照，把「刘海可用性」塞进去会让设置页之外的所有消费者都多一个字段。改为专用命令：

- `get_notch_player_state` → `{ available, enabled }`
- `set_notch_player_enabled { enabled }` → `{ available, enabled }`

`available=false` 时设置页渲染置灰开关 + 原因说明，且不回写偏好值；这样「合盖/换屏后不可用」不会把用户的选择清掉。

### D7 原型样式拆段后纳入生成流程

原型同时包含产品样式与验收台样式。把 `<style>` 拆成 `<style id="notch-product">` 与 `<style id="notch-demo">`，`extract-prototype-css.mjs` 增加第二个目标：只把 `#notch-product` 段逐字写到 `apps/desktop/src/styles/notch.css`。这样「视觉源在原型、生成物禁止手改」的既有约定对新窗口同样成立。

### D8 播放模式沿用中性色

`desktop-playback` 已经要求常驻播放栏的随机模式不得使用强调色；刘海播放器沿用同一结论：模式控件三种状态都是与上一首 / 下一首一致的中性色，只靠图标区分。强调色在面板里只出现在进度填充与播放·暂停键两处。

## Risks / Trade-offs

- **[窗口层级盖住菜单栏菜单]** → 使用 `NSStatusWindowLevel`（25）：高于菜单栏本身（24，否则收缩态会被菜单栏盖住），低于展开的系统菜单（`NSPopUpMenuWindowLevel` = 101，否则会挡住用户打开的菜单）。实测窗口层为 25，见 `docs/verification/macos-notch-player-2026-11-09.md`。
- **[resize 与 CSS 过渡不同步导致裁剪或闪烁]** → 展开先 resize 再动画；收起先动画再 resize；两处都在 Rust 侧保持顶边与水平中心不变。
- **[透明窗口 + vibrancy 在三平台 WebView 上表现不同]** → 该窗口只在 macOS 创建，Windows/Linux 无此代码路径；macOS 上仍做真机验证并保留退化方案（D4）。
- **[多显示器/合盖导致窗口留在错误屏幕]** → 监听 `NSApplicationDidChangeScreenParametersNotification`，重新计算可用性、屏幕与几何；不可用时移除窗口而不是留在原地。
- **[第二个 WebView 的内存与冷启动开销]** → 页面无曲库视图，只在需要时创建；偏好关闭或没有刘海时根本不创建。
- **[`macos-private-api` 的平台承诺]** → 该 feature 让 Tauri 打开私有 API 路径。仅影响需要透明窗口的代码路径，且 Echo 不经 App Store 分发；若将来需要上架，改用 `NSWindow.setOpaque(false)` + webview `drawsBackground` 的等价实现即可，规格不受影响。
- **[非激活窗口的鼠标事件穿透不足]** → 收缩态窗口只有 238×32，展开态窗口大于可见面板，四角透明区会吞掉点击；真机 Gate 明确检查「面板外的窗口区域不阻挡下方应用点击」，必要时对四角做 `setIgnoreCursorEvents` 分段处理。

## Migration Plan

- 无数据迁移：偏好字段缺失即默认开启，与用户预期一致；旧版本写回时通过 `RawDoc.unknown` 保留未知字段。
- 回滚：删除窗口创建路径即可；偏好字段留在本机文件里不影响旧版本（旧版本按未知字段保留）。

## Open Questions

- 展开态窗口比可见面板大（含外翻角与投影），四角透明区域的点击穿透在真机上的实际观感如何，需要 macOS 真机验收后再决定是否做分段穿透。
