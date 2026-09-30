## 1. 修复后台关闭生命周期

- [x] 1.1 以 AppKit `NSWindowDidExitFullScreenNotification` 确认全屏退出完成，再在主线程队列中隐藏主窗口。
- [x] 1.2 处理退出动画中的重复关闭请求；隐藏失败时保留 pending 状态并记录错误，正常窗口关闭仍立即隐藏。
- [x] 1.3 用户在退出动画期间主动重新打开主窗口时撤销待隐藏请求。

## 2. 验证

- [x] 2.1 运行 Rust 格式检查、macOS arm64 桌面 `cargo check` 和 Clippy。
- [x] 2.2 将绿键进入全屏、红键关闭的原生 UI 检查记录为 release DMG 验收项；当前 PR CI 不产出可安装发布包。
- [x] 2.3 在本机 release `.app` 中验证原生全屏关闭后 `is_visible() == false` 且 Echo 进程仍运行。
