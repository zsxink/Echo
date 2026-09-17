## 1. 播放会话与常驻播放栏

- [x] 1.1 扩展 `echo-desktop` 的本地播放会话序列化、写入和恢复流程以保存经校验的当前播放位置；在播放器就绪后恢复受时长约束的位置且保持暂停，确保前端先订阅恢复快照、暂停态立即呈现歌词/进度并可基于资料库时长拖动；补充会话缺失、损坏、越界和临时项的 Rust/React 测试，验证 `cargo test -p echo-desktop player::session` 通过。
- [x] 1.2 将桌面播放器的快照/上下文切换、后台关闭和显式退出接入节流后的最终会话写入，避免高频位置事件产生每次落盘；验证位置恢复及退出刷新相关 Rust 测试通过。
- [x] 1.3 修正非沉浸式传输控制的模式样式与无障碍选中语义：随机和单曲循环图标不采用主题强调色；播放模式不依赖当前歌曲、空播放态也可切换，并通过权威播放器快照立即回传；播放进度仅在已加载歌曲（包括暂停）时可拖动。补充 React/Rust 测试，并验证 `pnpm --filter @echo/desktop test -- --runInBand`（或 Vitest 等效定向命令）、`pnpm --filter @echo/desktop typecheck` 和相关 `cargo test -p echo-desktop` 通过。

## 2. 歌单选择器创建流程

- [x] 2.1 在歌曲“添加到歌单”选择器中连接现有 `PlaylistNameDialog` 和 `create_playlist` IPC 流程；成功后刷新权威歌单列表、选中新建歌单并保留原歌曲待确认，验证创建后确认添加的组件测试通过。
- [x] 2.2 为嵌套创建的校验/IPC 失败、取消和焦点恢复补齐测试，确保选择器、输入和既有选择不丢失且不出现伪成功；验证 `pnpm --filter @echo/desktop test` 通过。

## 3. 后台生命周期与产品名称

- [x] 3.1 在 Tauri 桌面壳实现基于关闭偏好的窗口关闭拦截：后台模式隐藏窗口并保留播放器，退出模式与显式退出保存会话、停止播放器并终止进程；处理托盘与 macOS Dock 重新打开以恢复隐藏主窗口，复用既有运行时玩家命令供后台控制调用，并以 Rust 测试验证状态转换与单实例行为。
- [x] 3.2 初始化并连接 macOS 菜单栏/Windows-Linux 托盘入口，提供显示 Echo、播放/暂停、上一首、下一首和退出；处理初始化失败时的可见退出路径，验证 `cargo test -p echo-desktop` 通过并完成目标平台手工冒烟。
- [x] 3.3 审核 Tauri 配置、窗口/托盘/菜单/通知文本、设置/关于页和打包显示字段，将用户可见产品名统一为 `Echo`，同时保留技术标识；验证构建产物与窗口/托盘文本的定向测试或人工检查。

## 4. 集成验证

- [ ] 4.1 运行桌面格式、静态检查、类型检查和测试：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test`。
- [x] 4.2 运行桌面构建与相关端到端/场景检查，验证随机样式、重启位置恢复、选择器新建并添加、后台关闭/托盘控制和 Echo 命名：`pnpm --filter @echo/desktop build`、`pnpm --filter @echo/desktop test:e2e`，并记录无法在当前主机自动覆盖的平台差异。
