## Why

用户反馈：macOS 主窗口进入系统原生全屏后点击红色关闭按钮，只退出全屏并恢复原窗口，窗口仍然可见。仓库已有 desktop-app-shell 规格要求全屏关闭后隐藏主窗口，但当前实现未稳定兑现该行为，需要修复窗口生命周期处理。

## What Changes

- 修复 macOS 后台关闭路径，确保原生全屏转换完成后主窗口被隐藏。
- 保留应用进程、菜单栏入口和播放会话。
- 让隐藏失败可观测且不会静默丢弃待处理关闭请求。

## Capabilities

行为已由 `openspec/specs/desktop-app-shell/spec.md` 定义，本次不新增或修改规格条款。

## Impact

影响 macOS Tauri 窗口生命周期处理；不改变跨平台产品行为、播放状态或设置语义。
