## 1. 修复后台关闭生命周期

- [x] 1.1 让全屏关闭请求在 native transition 完成后隐藏主窗口；隐藏失败时保留 pending 状态、记录错误，并以有界方式重试。
- [x] 1.2 保留 pending 状态已有的完成/取消覆盖；检查隐藏失败会保留 pending、定时重试只在窗口退出全屏后隐藏，并确认窗口模式关闭分支未改变。

## 2. 验证

- [x] 2.1 运行 Rust 格式检查、macOS arm64 桌面 `cargo check` 和 Clippy。
- [ ] 2.2 在生成的新 release DMG 上复现绿键进入全屏、红键关闭，并确认窗口隐藏、Echo 留在菜单栏且播放不中断；发布包需在 PR 合并后的 release 构建中验收。
