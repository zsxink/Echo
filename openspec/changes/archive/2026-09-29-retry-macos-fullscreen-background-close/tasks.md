## 1. 修复后台关闭生命周期

- [x] 1.1 让全屏关闭请求在 native transition 完成后隐藏主窗口；隐藏失败时保留 pending 状态、记录错误，并以有界方式重试。
- [x] 1.2 保留 pending 状态已有的完成/取消覆盖；检查隐藏失败会保留 pending、定时重试只在窗口退出全屏后隐藏，并确认窗口模式关闭分支未改变。

## 2. 验证

- [x] 2.1 运行 Rust 格式检查、macOS arm64 桌面 `cargo check` 和 Clippy。
- [x] 2.2 将绿键进入全屏、红键关闭的原生 UI 检查记录为合并后 release DMG 验收项；当前 PR CI 不产出可安装发布包。
