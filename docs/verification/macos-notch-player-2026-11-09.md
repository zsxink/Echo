# macOS 刘海播放器 — 真机验收记录

> 变更：`openspec/changes/add-macos-notch-player`
> 日期：2026-11-09
> 机器：MacBook Pro 16"（`Mac17,2`，Apple M5），macOS 27.2

## 1. 硬件探测（已实测）

用 `NSScreen.auxiliaryTopLeftArea` / `auxiliaryTopRightArea` 直接读取本机几何（与 `apps/desktop/src-tauri/src/notch/detect.rs` 相同的 API 与换算）：

```text
screen[0] frame=(0.0,0.0,1512.0x982.0) scale=2.0 main=true
  left=(0.0, 950.0, 663.5, 32.0) right=(848.5, 950.0, 663.5, 32.0)
  notch x=663.5 width=185.0 centre=756.0 screenCentre=756.0
screen[1] frame=(1512.0,-58.0,2048.0x1152.0) scale=2.0 main=false
  no auxiliary areas -> no camera housing
```

结论：

| 事实 | 值 | 对实现的影响 |
| --- | --- | --- |
| 内建屏 | 1512 × 982 pt @2x | 收缩窗口中心 756，与刘海中心重合 |
| 摄像头刘海宽度 | **185 pt** | 收缩态窗口宽 = 185 + 2 × 26 = **237 pt**（不是原型里的 238） |
| 菜单带高度 | **32 pt** | `COLLAPSED_HEIGHT = 32` 与真机一致 |
| 外接显示器 | 无辅助区域 | 「只有刘海屏才显示」的判定成立，外接屏不会误判 |
| 刘海居中 | 中心偏差 0 | `window_geometry` 以刘海中心对齐，而不是屏幕中心 |

这条实测直接否掉了「收缩态宽度写死」的做法：185 与原型假设的 186 不同，且不同机型刘海宽度不同。现在宽度由 `notch.width + 2 × EAR_WIDTH` 推出，渲染层通过 `--notch-hw-width` 拿到同一数值来画刘海与定位两侧内容。

## 2. 渲染层布局（已在浏览器实测）

`notch.html` 在 237 × 32 的窗口里加载真实组件与生成样式（`pnpm dev` + 无头 Chrome，4x 截图）：

- 黑色表面铺满窗口，刘海（185 pt）居中、两侧各 26 pt 窄耳；
- 左耳是 16 pt 封面占位（空播放态），右耳是静止的跳动音符（空播放态不跳动）；
- 窗口尺寸与 Rust 几何一致，没有溢出、没有滚动条。

## 3. 窗口层实测（绕开单实例移交）

本机已有一个 `/Applications/Echo.app` 实例在运行，`tauri-plugin-single-instance` 会把开发构建直接移交给它并退出（实测 `target/debug/Echo` 启动后立即以 0 退出、无任何窗口）。为了不动用户正在运行的版本，用 Tauri 的 `TAURI_CONFIG` 把构建临时指向另一个标识（`com.zsxink.echo.notchdev`，独立 app-data 目录），并配合内置的 `ECHO_GATE_QUIT_AFTER_MS` 自动退出跑了一次；已安装版本全程未受影响。

用 `CGWindowListCopyWindowInfo` 读取该进程的窗口，并用 `NSWindow.frame()` / Tauri `inner_size()` 交叉核对：

```text
window name=Echo layer=25 alpha=1.0 bounds=(x:639, y:0, w:233, h:32)
window name=Echo layer=0  alpha=1.0 bounds=(x:1958, y:16, w:1156, h:746)   ← 主窗口
NSWindow.frame()   = (637, 950, 237, 32)
Tauri inner_size() = PhysicalSize { width: 474, height: 64 }   ← 237 × 32 @2x
```

| 检查项 | 结果 |
| --- | --- |
| 窗口被创建 | ✅ 进程里多出一个 `Echo` 窗口 |
| 层级 | ✅ `layer = 25` = `NSStatusWindowLevel`：高于菜单栏本身（24），低于展开的系统菜单（`NSPopUpMenuWindowLevel` = 101） |
| 几何 | ✅ 237 × 32 pt（= 185 刘海 + 2 × 26 窄耳），顶边贴屏幕顶（CG y=0 / AppKit y=950、高 32） |
| 水平对齐 | ✅ 中心 755.5–756，与刘海中心 756 一致 |
| 主窗口不受影响 | ✅ 主窗口仍是 layer 0，位置尺寸未变 |
| 退出 | ✅ `ECHO_GATE_QUIT_AFTER_MS` 到点后进程正常结束，无残留 |

注：`CGWindowList` 报出的 233 pt 比 `NSWindow.frame()` 的 237 pt 窄 4 pt（左右各 2 pt），是 macOS 26+ 对窗口边界（圆角/透明边缘）的报告口径差异；Tauri 的 `inner_size` 与 `NSWindow.frame()` 都是 237 × 32，渲染层的 CSS 视口因此是 237 pt。

## 4. 尚未完成的真机目视验收（需要先退出正在运行的 Echo）

`CGWindowList` 只能读到窗口的几何与层级；本机终端没有「屏幕录制」权限，`screencapture` 只能拿到全黑图，因此**画面与交互**仍需人工过一遍。在退出已安装版本后跑一次：

```bash
pnpm --dir apps/desktop build
cargo build -p echo-app
./target/debug/Echo
```

| # | 检查项 | 对应 spec 场景 |
| --- | --- | --- |
| 1 | 刘海两侧出现收缩态（纯黑、左封面、右跳动音符），播放中跳动、暂停静止 | 收缩态内容 / 播放与暂停 |
| 2 | 指针进入窄耳展开、移出收起；指针只经过刘海本体不展开 | 悬浮展开 / 移出收起 / 指针经过硬件刘海 |
| 3 | 展开态出现封面、歌名、歌手、进度条与五个控制；进度条可拖动且拖动中不收起 | 展开态内容 / 拖动进度条 |
| 4 | 展开态面板后方是**真实桌面模糊**（`NSVisualEffectView`），不是纯色 | 视觉基准（液态玻璃） |
| 5 | 点击面板上任意控件时 Echo 不成为前台应用、焦点不离开当前应用 | 点击刘海播放器 |
| 6 | 不出现在 Dock / `⌘Tab` / Mission Control；切换桌面后仍在 | 不干扰系统 |
| 7 | 全屏应用下可见且不遮挡菜单栏状态项 | 全屏应用与菜单栏 |
| 8 | 展开态窗口四角透明区不吞掉下方应用的点击 | Risks：鼠标事件穿透 |
| 9 | 设置「应用行为」出现「刘海播放器」开关，可切换且即时生效；重启后保持 | 偏好 / 关闭并重启 |
| 10 | 合盖或断开内建屏后窗口消失、开关置灰但不清空已保存值；重新打开内建屏后恢复 | 没有刘海屏时置灰 / 屏幕参数变化 |
| 11 | 与主播放栏、菜单栏状态项、系统媒体键双向同步 | 从刘海播放器控制播放 / 从其它入口控制播放 |
| 12 | 播放模式三态只换图标、不变色 | 播放模式不使用强调色 |

## 5. 已由自动化覆盖的部分

| 层 | 证据 |
| --- | --- |
| 几何 | `notch::geometry` 单元测试 7 项：居中、贴顶边、刘海不居中、刘海过宽时窗口加宽、近边缘钳制、副屏负坐标、窄屏兜底 |
| 状态机 | `notch` 单元测试 2 项：偏好与可用性互不覆盖 |
| 偏好持久化 | `platform::local_state` 新增 4 项：默认开、往返、错类型只回退该字段、读取不改写 |
| 渲染层 | `NotchPlayer.test.tsx` 18 项：空态/临时项/无封面、跳动音符与播放态、模式三态无色、展开协议（先 resize 再动画、收起后延迟缩窗、拖动中不收起）、刘海宽度注入 |
| 设置页 | `SettingsView.test.tsx` 新增 5 项：有刘海可用、无刘海置灰且保留值、置灰时点击不发命令、切换后采纳回值、非 macOS 不渲染 |
| 样式来源 | `prototype-css-drift` 门：6 份生成样式与两份原型逐字一致 |

## 6. Scenario 交叉核对（task 7.3）

逐条对照两个 delta spec 的每个 Scenario，标注实现落点与证据。「真机」表示需要第 3 节的实测补证。

### macos-notch-player

| Scenario | 实现落点 | 证据 |
| --- | --- | --- |
| 带刘海屏幕且偏好开启 | `bootstrap.rs` setup 末尾 `NotchPlayer::sync` → `notch::window::ensure` | 真机 #1 |
| 没有刘海屏 | `detect::detect()` 返回 `None` → `sync` 走 `remove_window` | 实测外接屏无辅助区域；真机 #10 |
| 非 macOS 平台 | `notch` 模块只在 `#[cfg(target_os = "macos")]` 编译 `detect`/`window`；`NotchPlayer::detect_available` 非 macOS 恒 false | `cargo check` 于非 macOS 目标无该代码路径；设置页测试「非 macOS 不渲染」 |
| 屏幕参数变化 | `notch::window::install_screen_observer`（`NSApplicationDidChangeScreenParametersNotification`）→ `NotchPlayer::refresh_screen` → `sync` 重定位或移除 | 真机 #10 |
| 偏好关闭 | `set_notch_player_enabled` → `notch.set_enabled` + `sync` → `remove_window` | `SettingsView.test`「切换后采纳回值」；真机 #9 |
| 收缩态内容 | `NotchPlayer.tsx` `.ears` + `notch.css`（宽度 `calc(var(--notch-hw-width) + 52px)`） | 浏览器实测：237 × 32 布局；`NotchPlayer.test`「临时项展示名」 |
| 展开态内容 | `NotchPlayer.tsx` `.panel`（封面/歌名/歌手/进度/五控制） | `NotchPlayer.test` 展开协议；真机 #3 |
| 没有当前播放项 | `data-empty` 由 `currentQueueEntryId === null` 驱动 | `NotchPlayer.test`「不残留旧封面/旧歌名」 |
| 封面不可用 | `useCoverKeys` 无 key → 占位；`onError` 记 `failedCover` | `NotchPlayer.test`「空态无 img」 |
| 播放与暂停 | `data-playing = state === "playing" && hasCurrent`；CSS 动画只挂在该属性上 | `NotchPlayer.test` 三项（playing/paused/loading） |
| 切歌 | 快照驱动，`coverKey` 与标题同帧更新 | `NotchPlayer.test`「下一首换歌名与封面」 |
| 减少动画 | `notch.css` 的 `@media (prefers-reduced-motion)` 把动画时长/延迟归零 | 原型验收台实测 `animation: none` |
| 悬浮展开 | 根容器 `onPointerEnter`（整条，含刘海本体）→ `await notch_player_set_expanded(true)` → 置 `data-expanded` | `NotchPlayer.test`「awaits the resize before showing」 |
| 移出收起 | `onPointerLeave` → 90 ms 后收起动画，420 ms 后缩窗 | `NotchPlayer.test`「collapses on leave …」 |
| 指针经过硬件刘海 | 根容器 `onPointerEnter`：整条收缩态（含刘海本体）都是命中区；`.hardware-notch` 为 `pointer-events: none`，事件落到容器 | `NotchPlayer.test`「expands when the pointer is over the housing strip itself」；真机 #2 |
| 拖动进度条 | `onPointerDown` 置 `draggingRef`，`collapse` 直接返回 | `NotchPlayer.test`「stays open while dragging」 |
| 从刘海播放器控制播放 | `player_control` / `seek` / `set_favorite` 与播放栏同命令 | `NotchPlayer.test`「sends the same transport actions」 |
| 从其它入口控制播放 | 同一 `player://snapshot` 流 | `playerStore` 单一边界；真机 #11 |
| 播放模式不使用强调色 | `playbackModes.ts` 只含 label/next/icon；`notch.css` 无 `#btn-mode` 颜色声明 | `NotchPlayer.test`「never gives the mode control a theme-accent colour rule」 |
| 临时播放项 | `data-temporary` + 喜欢禁用 | `NotchPlayer.test`「disables the favourite control for a temporary item」 |
| 拖动进度 | `seek` 命令 + 同一位置流 | `NotchPlayer.test`「seeks with the slider's value」 |
| 点击刘海播放器 | 窗口 borderless + `NSStatusWindowLevel` → `canBecomeKeyWindow` 为 false | 真机 #5 |
| 全屏应用与菜单栏 | `collectionBehavior` 含 `FullScreenAuxiliary` + `Stationary`；层级低于系统菜单 | 真机 #6/#7 |
| 键盘与辅助技术用户 | 该窗口不承接焦点；同等操作保留在播放栏/状态项/媒体键 | 设计约束（spec R6）；真机 #5 |
| 默认开启 | `DEFAULT_SHOW_NOTCH_PLAYER = true` | `local_state` 测试「defaults to on when absent」 |
| 关闭并重启 | `showNotchPlayer` 持久化 | `local_state` 测试「round trips」；真机 #9 |
| 没有刘海屏时置灰 | `SettingsView` `disabled={available !== true}` + 原因文案 | `SettingsView.test`「greys the switch out …」 |
| 偏好损坏 | 逐字段宽容解析，错类型只回退该字段 | `local_state` 测试「wrong type degrades only itself」 |
| 位置推进 | 封面按 key 缓存，位置帧不带封面 | `useCoverKeys` 既有行为；`NotchPlayer` 不做逐帧封面读取 |
| 切歌换封面 | `coverKey` 变化 → 新 `assetUrl(key, "list")` | `NotchPlayer.test`「下一首换歌名与封面」 |

### desktop-app-shell（delta）

| Scenario | 实现落点 | 证据 |
| --- | --- | --- |
| 切换主题 / 下次启动恢复主题 / 偏好不可保存或值非法 | 既有实现未改动 | 既有 `local_state` 与设置页测试 |
| 刘海播放器偏好按平台与硬件呈现 | `SettingsView` 的 `isMacos` 分支 + `available` 置灰 | `SettingsView.test` 五项 |
| 刘海偏好与不可用状态互不覆盖 | `set_notch_player_enabled` 在 `!available` 时直接返回、不写偏好；`load` 从不回写 | `local_state` 测试「keeps the value on disk when unavailable」；`SettingsView.test`「greys … keeps the stored value」 |
| 重复启动应用 / 系统文件关联 / 首实例尚未就绪 / 监听器晚于请求 / 运行中收到请求 | 既有单实例与文件打开通道未改动 | 既有测试 |
| 伴随窗口不构成第二个实例 | 刘海窗口同进程、无独立托盘/播放器；单实例插件只看进程 | `notch` 模块不创建 player/queue；真机 #11 |
