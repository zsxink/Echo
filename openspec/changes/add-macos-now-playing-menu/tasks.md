## 1. macOS MediaPlayer 边界

- [x] 1.1 在 `apps/desktop/src-tauri` 新增 macOS-only MediaPlayer 适配器及必要框架链接/绑定配置，且不向 `echo-core` 或非 macOS 目标泄漏平台类型；验证 macOS `cargo check -p echo-app` 和非 macOS workspace 检查均通过。
- [x] 1.2 实现安全的 Now Playing 投影，复用现有协调器、歌曲元数据与封面缓存，生成标题、艺人、专辑、时长、有效进度、状态和可选 artwork；验证无封面、缺失字段、临时项、空会话及绝对路径不泄漏的 Rust 单元测试通过。

## 2. 系统状态发布

- [x] 2.1 在播放器组合根订阅唯一播放会话，将切歌、播放/暂停、seek、自然结束、加载失败与会话恢复投影到 `MPNowPlayingInfoCenter`；对位置更新限流并以 entry identity/revision 丢弃过期封面结果，验证状态转换与快速切歌的测试通过。
- [x] 2.2 在空/停止会话和显式退出时清除 Echo 发布的系统状态，并在恢复时发布暂停的恢复项和归一化进度；验证退出、恢复和无当前项不显示陈旧歌曲的集成测试或 macOS 手工验收记录完成。

## 3. 系统远程命令

- [x] 3.1 只注册一次 macOS 播放、暂停、上一首和下一首远程命令，并通过既有类型化命令/协调器路径执行；验证每类系统命令只触发一次既有播放会话动作的单元或集成测试通过。
- [x] 3.2 对没有可播放项、无可用上一首/下一首或播放器拒绝命令返回如实的失败/不支持结果；验证系统命令不会创建第二个播放器、窗口或队列的测试通过。

## 4. 平台回归与交付验证

- [x] 4.1 在 macOS 手工验证：播放歌曲后系统 Now Playing 可显示 Echo 信息（由系统决定布局/展示时机），封面与标题正确，控制中心和媒体键的播放、暂停、上一首、下一首与主窗口同步；记录结果。
- [x] 4.2 验证既有 `NSStatusItem` 菜单栏控制仍可用，且 Windows/Linux 原托盘路径未变；运行 `cargo test --workspace --all-features` 并确认通过。
- [ ] 4.3 执行交付门禁：运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test -- --run` 与 `openspec validate add-macos-now-playing-menu --strict`，并修复本变更导致的失败。

