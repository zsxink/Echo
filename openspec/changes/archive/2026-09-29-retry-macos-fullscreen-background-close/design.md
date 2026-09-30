## Context

PR #43 增加了全屏退出后的延迟隐藏：收到后台关闭请求时标记待隐藏、调用 `set_fullscreen(false)`，并在后续 `Resized` 事件确认窗口已退出全屏后隐藏。随后 PR #46 增加定时重试，但用户从该分支构建并安装 DMG 后仍复现窗口恢复原尺寸且保持可见。Tao 的 `set_fullscreen(false)` 在 AppKit 动画完成前先更新内部全屏标记，因此动画期间的 `Resized` 会误判为已退出全屏，并提前清除待隐藏状态；AppKit 最后恢复窗口时又将其显示出来。

## Goals / Non-Goals

**Goals:** 以 AppKit 原生退出完成通知作为隐藏条件，可靠完成后台关闭；隐藏错误必须被记录，待处理动作只能在隐藏成功后清除。

**Non-Goals:** 不改变全屏进入/退出按钮语义，不退出 Echo 进程，不调整播放和菜单栏行为，不改变 Windows/Linux。

## Decisions

- 保留 CloseRequested → 退出全屏 → native transition 完成 → 隐藏的状态顺序，避免在动画期间直接隐藏而重现黑屏。
- 在 Tauri 的 `RunEvent::Ready`（配置窗口创建完成后）只观察主 `NSWindow` 的 `NSWindowWillExitFullScreenNotification` 与 `NSWindowDidExitFullScreenNotification`。前者标记原生退出正在进行；后者从工作线程投递隐藏任务到主线程队列。Tauri 在主线程调用 `run_on_main_thread` 会同步执行，因此必须从工作线程投递，确保 Tao 完成同轮事件分发后再隐藏。
- 移除 `Resized` 和基于时间的重试作为完成依据；Tao 的缓存状态不能证明 AppKit 动画完成。退出动画中再次收到关闭请求时，仍等待原生完成通知。
- 发起全屏退出时立即标记原生转换进行中，覆盖 `WillExit` 通知尚未送达前的重复关闭请求。仅在 `window.hide()` 成功后清除待处理状态；隐藏失败时记录错误，后续关闭请求可再次尝试。
- Dock、菜单栏、文件打开及第二实例等显式唤醒主窗口的入口先撤销待隐藏请求；原生退出动画标记保留到 `DidExit`，避免新的关闭请求绕过等待。
- 规格复用当前 `desktop-app-shell` requirement；本 change 仅修复实现与既有规格的偏差，因此 `skip_specs: true`。

## Risks / Trade-offs

- macOS 全屏退出由原生动画异步完成；通知观察器必须在应用运行期间存活，并在退出时注销。
- 若 AppKit 未发出退出完成通知或 `hide()` 返回错误，窗口会保持可见并留下日志；不再靠猜测动画时长强行隐藏。
- 对已隐藏窗口执行可访问性读取会使 macOS 发送 `Reopen` 并主动唤回主窗口；验收须在不再次激活应用的前提下观察隐藏结果。

## Migration Plan

无需数据迁移或配置变更；更新窗口生命周期处理与对应回归验证即可。

本地 release `.app` 已验证：绿色按钮进入原生全屏后触发原生关闭请求，AppKit 依次发出 `WillExit`、`DidExit`，随后窗口 `is_visible() == false`，Echo 进程仍运行。生成包含修复的 release DMG 后，还需在安装应用中以红色按钮手工验证主窗口隐藏、菜单栏入口与播放会话。当前 PR 的 CI 不生成可安装 release DMG，因此这一步属于发布验收。
