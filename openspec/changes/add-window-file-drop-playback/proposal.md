# Proposal

## Why

[#117](https://github.com/zsxink/Echo/issues/117) 记录了 Echo 窗口对系统文件管理器的文件投放完全没有响应：既不播放也无反馈，用户只能靠猜。而「打开资料库外文件并立刻播放」的链路（`deliver_file_open` → `app://file-open-request` → `play_temporary_file`）本就存在且本来就自动播放，只是入口只有 OS 文件关联。该链路还有两个既有缺陷：前端对每个路径逐个调用 `play_temporary_file`，而 `coordinator::play_temporary` 每次清空队列只 push 一项，一次打开 N 个文件只剩最后一个（#4）；且每次都替换当前队列并清空随机轮记录（与 #5 相关）。「一次拖入一整张专辑」是拖放的主场景，会把前者从边角放大成主路径。

#117 的产品决策已在 [issue 评论](https://github.com/zsxink/Echo/issues/117#issuecomment-6723593067) 拍板：**Q1 拖放只播放、不入库；Q2 不接受文件夹；Q3 选 A —— 严格照搬双击打开的队列语义**；Q4（临时项标注）与 Q5（浮层材质复用 #74）按提案默认。方案前提是拖放只新增一个入口，不新增播放路径。

## What Changes

- 在主窗口启用窗口级文件投放捕获：文件拖入窗口范围即出现覆盖整窗的「松手即播放」反馈层，拖离即消失；松手后自动播放，无确认弹窗、无需再点播放键。
- 拖放严格复用既有 file-open 通道，与双击音频同语义：活动资料库内文件走既有 UUID，库外文件建当前会话临时项；替换当前队列与随机轮记录（Q3=A），**不**引入追加到队尾、「下一首播放」或可切回的临时队列等新语义，不改 #5 的优先区逻辑。
- 修 #4：一次投放（以及一次系统打开）N 个文件按输入顺序进入**同一条**队列、从第 1 首播放，不再逐个重建队列、不再只剩最后一个；库内/库外混合时保持输入顺序。
- 拒绝不支持的投放：按 Core `SUPPORTED_EXTENSIONS` 过滤单个音频文件；文件夹、非音频给出明确可读反馈，不静默无反应；不递归、不取目录顶层、不视为「添加扫描目录」（Q2）。
- 固化边界：不入库（不复制 `media/`、不写 SQLite/歌单/播放统计/同步队列，入库仍走 `import_current_temporary_file`）、路径只解码一次、冷启动与加载中排队、资料库不可用或只读时仍可播、不扩 capability 权限面、反馈层不挡标题栏拖拽条与 Windows 自绘窗口按钮、不打乱 `OverlayTier` 与 Escape 逐层退出、`prefers-reduced-motion` 去动效但保留状态变化、临时项不入会话恢复与统计、三平台一致。
- 界面基准与术语：原型补投放反馈层并重跑 `node scripts/extract-prototype-css.mjs`（生成物禁止手改），`docs/interface-terminology.md` 补「投放」概念与「临时」标注，`docs/PRODUCT.md` 补拖放入口说明（Q4/Q5）。

**非目标**：不实现文件夹投放与「添加扫描目录」；不提供「播放 / 导入」双落点或第二手势（Q1）；不改变队列替换语义（Q3=A）；不做歌单内拖拽排序（`playlist-management`）；不改 #6/#9 之外的 open-file 状态机范围；不把播放能力或拖放逻辑放进 Core；不新增第三方依赖、不引入 `fs` / `shell` / `dialog` 权限族；不做移动端。

## Capabilities

### New Capabilities

（无。）沿用现有扁平 capability 组织：窗口级投放属于 `desktop-app-shell`，播放队列属于 `desktop-playback`，外部文件安全边界属于 `safe-file-ingestion`。不新建能力，避免近重复目录，也避免给 `scripts/verify/spec-scenarios.mjs` 引入新的前缀映射。

### Modified Capabilities

- `desktop-app-shell`: 新增窗口级投放的捕获与反馈、松手即播、可读拒绝、路径与到达时机、三平台一致性要求。
- `desktop-playback`: 修改「临时播放项边界」，补一次外部打开/投放多个文件按序建立单一队列并替换当前队列的契约与场景。
- `safe-file-ingestion`: 修改「源文件、系统关联与安全边界」，把窗口投放纳入外部文件打开入口，明确不入库、不改源文件、不二次解码。

## Impact

- **事实来源**：`docs/PRODUCT.md`「支持将 Echo 设为已支持音频格式的默认打开应用」与 ROADMAP 一期验收「资料库外音乐可由系统双击直接临时播放，不会被加入资料库」是语义基准；`docs/DESIGN.md` 的平台适配分层与 `openspec/CODE_STANDARDS.md` §3、§6、§8、§9 是约束；界面以 `docs/prototype/echo-desktop-player.html` 与 `docs/interface-terminology.md` 为准。
- **壳层（Presentation / Platform）**：`apps/desktop/src-tauri/tauri.conf.json` 与 `tauri.windows.conf.json` 显式声明拖放开关；`bootstrap.rs` 注册窗口拖放事件处理；`file_open_gate.rs` 支持批量投递并新增一条只传状态的投放反馈事件。Core 领域模型不变。
- **播放层（echo-desktop）**：`crates/echo-desktop/src/player/coordinator.rs` 增加一次建队列的批量入口；`commands.rs` 以批量命令 `play_opened_files` 替换逐 path 的 `play_temporary_file`。command 名称与参数变化属接口变更：唯一消费者是 `App.tsx`，无外部兼容负担，但必须同步 `bootstrap.rs` 注册、`crates/echo-desktop/src/ipc/generate.rs`、`apps/desktop/src/ipc/ipc-types.generated.ts`（生成物只经生成器更新）与 `apps/desktop/src/bridge/index.ts`，并清理旧命令的全部引用与测试。
- **前端**：`apps/desktop/src/app/App.tsx` 订阅改为一次批量调用并新增投放状态订阅；新增投放反馈组件与样式；原型与其 CSS 生成物同步。
- **规格 / 门禁**：`docs/PRODUCT.md`、`docs/interface-terminology.md`、`docs/traceability.md`、`scripts/verify/scenario-commands.mjs`、`scripts/verify/manifest.json`（由生成器重建）按 §9 登记新场景；新增场景追加在各 capability 末尾，禁止重排既有稳定 ID。
- **无数据迁移**：不涉及 SQLite schema、远端协议或持久化格式；回滚只需恢复旧命令、移除拖放订阅并恢复窗口配置。
- **验证基线**：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-targets --all-features`；`pnpm --filter @echo/desktop format:check` / `lint` / `typecheck` / `test -- --run` / `build`；`node scripts/extract-prototype-css.mjs`；`pnpm verify:governance`；`pnpm verify:scenario -- --all`；`openspec validate --strict`。
