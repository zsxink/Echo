# Tasks

## 1. 原型样式拆分与生成物

- [x] 1.1 把 `docs/prototype/echo-macos-notch-player.html` 的 `<style>` 拆成 `<style id="notch-product">`（tokens + 播放器与面板样式）与 `<style id="notch-demo">`（验收台、页面框架、模拟桌面），产品段内不得残留任何验收台选择器。验证：`node scripts/extract-prototype-css.mjs` 跑通后，生成的 `apps/desktop/src/styles/notch.css` 中 `grep -E "demo-|page-head|stage|notes|mock-window"` 无匹配
- [x] 1.2 扩展 `scripts/extract-prototype-css.mjs`，增加 notch 目标：只拷贝 `#notch-product` 段到 `apps/desktop/src/styles/notch.css`，并在文件头写入与其他生成物一致的「禁止手改」说明。验证：脚本连续运行两次产物字节一致（幂等），且 `git status` 中主原型生成的 `styles/*.css` 无变化
- [x] 1.3 在治理门登记新的样式生成物与漂移校验，确保手改 `notch.css` 会被发现。验证：`pnpm verify:governance` 通过；手工改动一行 `notch.css` 后该命令失败，还原后恢复通过

## 2. 本机偏好与 IPC

- [x] 2.1 `crates/echo-desktop/src/platform/local_state.rs` 增加 `show_notch_player: Option<bool>`（`RawDoc`）与 `show_notch_player: bool`（`ResolvedState`，默认 `true`），逐字段宽容解析并沿用 `had_corruption`；增加 `set_show_notch_player` / `show_notch_player` 访问器。验证：`cargo test -p echo-desktop --lib platform::local_state` 覆盖「缺失取默认」「true/false 往返」「错类型只回退该字段且不破坏兄弟字段」「损坏文件取默认」
- [x] 2.2 在 `crates/echo-desktop/src/ipc/generate.rs` 增加 `get_notch_player_state` / `set_notch_player_enabled` 的返回类型（`{ available: boolean; enabled: boolean }`），随后 `pnpm --dir apps/desktop generate:ipc` 重生成 `ipc-types.generated.ts`。验证：重生成后 `git diff` 只包含新增类型，且再跑一次生成无 diff
- [x] 2.3 在 `apps/desktop/src-tauri/src/commands.rs` 实现两条命令：`get_notch_player_state` 与 `set_notch_player_enabled`，`available` 由 macOS 刘海检测提供，非 macOS 恒为 `false`；`set_*` 在 `available=false` 时不得回写偏好。验证：`cargo test -p echo-desktop` 与 `cargo test -p echo-desktop-app`（或等价 crate）覆盖「非 macOS 恒不可用」「不可用时写入不改变已存值」

## 3. 刘海检测与窗口几何

- [x] 3.1 新增纯几何模块（不依赖 AppKit），输入「屏幕 frame + 刘海左右两侧矩形 + 目标状态」，输出窗口 rect；收缩态 238×32、展开态 412×196，顶边贴屏幕顶边，水平以刘海中心对齐。验证：单元测试覆盖单屏居中、刘海不居中、Retina 2x、多屏只选带刘海屏、无刘海返回 `None`
- [x] 3.2 新增 `apps/desktop/src-tauri/src/macos_notch.rs`：用 `NSScreen.auxiliaryTopLeftArea` / `auxiliaryTopRightArea` 实现 `detect()`，返回刘海矩形与所在屏幕，并在 `Cargo.toml` 为 `objc2-app-kit` 补 `NSScreen` / `NSWindowCollectionBehavior` / `NSVisualEffectView` feature。验证：`cargo check -p echo-desktop-app --target aarch64-apple-darwin` 通过；真机日志输出与 `system_profiler` 的刘海机型一致（记录到 6.2 的真机验收记录）
- [ ] 3.3 监听 `NSApplicationDidChangeScreenParametersNotification`，屏幕参数变化时重新执行 3.2 与 3.1 并广播给窗口管理逻辑。验证：真机接入/断开外接显示器时窗口重定位或消失（记录到 6.2）

## 4. 伴随窗口与壳集成

- [x] 4.1 程序化创建 `notch` 窗口：`transparent` + `decorations: false` + `shadow: false` + `resizable: false` + `focus: false` + `skipTaskbar`；AppKit 后处理设置 `NSWindowStyleMaskBorderless`、`canBecomeKeyWindow = false`、`NSStatusWindowLevel`、`collectionBehavior`（`canJoinAllSpaces` / `stationary` / `ignoresCycle` / `fullScreenAuxiliary`）。验证：真机启动后窗口出现且 Echo 不成为前台应用、不出现在 Dock 与 `⌘Tab`、切换桌面后仍可见（记录到 6.2）
- [x] 4.2 给窗口背景挂 `NSVisualEffectView`（`blendingMode = .behindWindow`，`material = .hudWindow`），WebView 背景透明，使展开态的液态玻璃能模糊真实桌面。验证：真机截图对照原型展开态，确认面板后方的桌面内容被模糊而非纯色（记录到 6.2）
- [x] 4.3 实现 `notch_player_set_expanded` 命令：按 3.1 计算新 rect，先 `set_size` 再 `set_position`（保持顶边与水平中心不变），并拒绝在窗口不存在时执行。验证：几何单元测试 + 真机展开/收起无位移跳动（记录到 6.2）
- [x] 4.4 把偏好与可用性接到窗口生命周期：可用且开启时创建并显示，偏好关闭或不可用时销毁/隐藏；启动时按 `show_notch_player` 决定是否创建。验证：`cargo test` 覆盖状态机（可用×开启 四种组合），真机切换开关即时生效（记录到 6.2）
- [x] 4.5 新增 `apps/desktop/src-tauri/capabilities/notch.json`，`windows: ["notch"]`，只给 `core:default` 与窗口自省所需的最小集合，不引入 `fs` / `shell` / `dialog` / `process` 权限族。验证：`pnpm verify:governance` 通过；capability 文件内 `grep -E "fs:|shell:|dialog:|process:"` 无匹配

## 5. 伴随窗口前端与设置开关

- [x] 5.1 新增 `apps/desktop/notch.html` 与 `apps/desktop/src/notch.tsx` 入口，在 `vite.config.ts` 的 `rollupOptions.input` 注册；入口订阅既有 `player://snapshot` / `player://queue`，复用 `playerStore`、`useCoverKeys`、`useSongDetail`、`PlayerProgress` 与既有 `player_control` / `seek` / 收藏命令，不新增播放通道。验证：`pnpm --dir apps/desktop build` 产出 `dist/notch.html`；vitest 断言入口渲染后只调用既有命令名
- [x] 5.2 实现收缩态与展开态视图：收缩态左封面缩略图（`assetUrl(key, "list")`）、右跳动音符；展开态封面 + 歌名 + 歌手 + 音符 + 进度条 + 五个控制；空播放态与临时播放项按常驻播放栏语义。验证：`pnpm --dir apps/desktop test -- notch` 覆盖「无当前项显示占位且不残留旧歌名」「临时项喜欢禁用」「无封面用占位」
- [x] 5.3 实现跳动音符状态与减少动画：复用 `.playing-bars` 语义，仅在播放中动画，暂停/停止/加载失败静止，`prefers-reduced-motion` 下静止。验证：vitest 断言 `data-playing` 与 `animation-name` 的对应关系
- [x] 5.4 实现悬浮展开/收起协议：指针进入收缩态可交互区时 `await` `notch_player_set_expanded(true)` 后再置展开态；移出先播放收起过渡再调用 `false`；进度条拖动与控件操作期间不收起；指针只经过硬件刘海不触发展开。验证：vitest 用假的 bridge 断言调用顺序与「拖动中不收起」
- [x] 5.5 播放模式控件三种状态都不使用主题强调色（与上一首/下一首同色，仅图标区分）。验证：vitest 断言三态的计算颜色一致且不等于强调色 token
- [x] 5.6 `apps/desktop/src/features/settings/SettingsView.tsx` 增加 macOS 专属「刘海播放器」开关行：`available=false` 时 `disabled` 并显示原因文案，不回写偏好；Windows/Linux 不渲染该行。验证：`pnpm --dir apps/desktop test -- SettingsView` 覆盖「有刘海可切换」「无刘海置灰且点击无效」「非 macOS 不渲染」
- [x] 5.7 更新 `docs/interface-terminology.md`（新增「刘海播放器」「收缩态」「展开态」术语与两态可见内容），并在 `docs/prototype/` 说明该原型是本窗口的视觉事实来源。验证：术语表中的名称与组件 `aria-label` 一致（人工核对并记录在 6.2）

## 6. 文档与真机验收

- [x] 6.1 更新 `docs/PRODUCT.md`（桌面运行上下文新增刘海播放入口与「仅带刘海屏可用」约束）与 `docs/ROADMAP.md`（一期能力清单）。验证：文档描述的默认值、置灰条件与 spec 一致（人工核对）
- [ ] 6.2 记录 macOS 真机验收：有刘海机型出现/无刘海机型不出现、外接显示器与合盖、全屏应用与菜单栏、点击不激活应用、展开态 vibrancy 模糊真实桌面、窗口透明区不阻挡下方点击、偏好开关即时生效与重启保持。验证：`docs/verification/` 下新增记录文件，逐项附截图或屏幕录制说明

## 7. 集成验证

- [x] 7.1 运行 `pnpm --dir apps/desktop typecheck`、`pnpm --dir apps/desktop lint`、`pnpm --dir apps/desktop test`、`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`。验证：全部通过
- [x] 7.2 运行 `pnpm verify:governance` 与 `pnpm verify:task -- 1.9 9.7`（涉及新增 capability 与样式生成物）。验证：全部通过
- [x] 7.3 交叉检查实现与两个 delta spec：逐条确认 `macos-notch-player` 与 `desktop-app-shell` 的每个 Scenario 都有对应实现或真机记录，缺口回填。验证：核对清单写入 6.2 的记录文件
