# Tasks

## 1. 技术验证与基线

- [x] 1.1 复核 Tauri 2.11.6 的 `dragDropEnabled` 默认值与 `DragDropEvent` 四个变体（读 `~/.cargo/registry/src/.../tauri-utils-2.10.2/src/config.rs` 与 `tauri-runtime-2.11.3/src/window.rs`），确认「显式 `true` 才会生成壳层事件」的结论，并把核对结果记入 design 的 Context；验证：`cargo tree -p echo-app -i tauri` 与源码引用写入审阅记录，两处窗口配置字段逐项比对无漂移。
- [ ] 1.2 三平台最小投放验证：在 macOS / Windows / Linux（含 WebKitGTK）上分别记录 `Drop` 事件给出的 `paths` 是否为本地绝对路径、多文件顺序是否与系统选择顺序一致、百分号编码是否已由 wry 解码；验证：三份证据日志落到 `artifacts/`，若 Linux 拿不到可用 `paths` 则按 design D1 回退预案更新 design 与任务组 2，规格不变。
- [x] 1.3 记录场景 ID 基线，确认三个 delta 的新 requirement / scenario 都追加在末尾、不重排既有稳定 ID；验证：`node scripts/verify/spec-scenarios.mjs --check` 输出与 `docs/traceability.md` 现有 577 个 ID 完全一致。
- [x] 1.4 定位 Core `SUPPORTED_EXTENSIONS` 的既有复用点与临时元数据读取入口，确认壳层过滤无需新常量表、且不触碰写入门；验证：`rg -n "SUPPORTED_EXTENSIONS|guard_writes" crates/echo-core crates/echo-desktop apps/desktop/src-tauri` 的引用清单写入审阅记录。

## 2. 壳层：窗口配置与投放捕获

- [x] 2.1 在 `apps/desktop/src-tauri/tauri.conf.json` 与 `tauri.windows.conf.json` 的主窗口定义显式写出 `dragDropEnabled: true`，Windows 配置除 `decorations` 外仍与基础配置一致；验证：`cargo build -p echo-app` 通过，且 `rg -n "dragDropEnabled" apps/desktop/src-tauri/*.json` 两处取值相同。
- [x] 2.2 在 `bootstrap.rs` 构建链注册 `on_window_event`，匹配 `Enter` / `Over` / `Drop` / `Leave` 并发出只含状态的 `app://file-drop-state`；验证：新增 `echo-app` 单元测试覆盖 enter→active、leave→inactive、事件 payload 不含路径，运行 `cargo test -p echo-app --all-features`。
- [x] 2.3 把 `file_open_gate.rs::deliver_file_open` 扩为批量投递，保持 `StartupSupervisor` FIFO、`file_open_frontend_ready` 双门与 `record_gate_open` 的一次性记录语义；验证：新增 Rust 测试覆盖「监听器未就绪 → 暂存 → 就绪后按到达顺序补发」与「一次 Drop 只发一个事件」，运行 `cargo test -p echo-app --all-features`。
- [x] 2.4 在壳层实现投放过滤与可读拒绝：`is_dir()` → 文件夹拒绝、扩展名按 Core 支持矩阵 → 非音频拒绝、混合投放保留受支持项相对顺序，不排序不去重；验证：新增 Rust 表驱动测试覆盖 8 种支持扩展名、非音频、文件夹、混合投放与顺序保持，运行 `cargo test -p echo-app --all-features`。
- [x] 2.5 本组静态检查与回归；验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test -p echo-app --all-features` 全部通过。

## 3. 播放层：一次建队列与 IPC 接口变更

- [x] 3.1 在 `crates/echo-desktop/src/player/coordinator.rs` 增加平台层枚举（库内 `SongId` / 库外 `TemporaryPlay`）与批量入口：一次 `replace_queue`、按输入顺序 push N 个 `QueueEntry`、`set_current` 第一个、清 `failed_round`、`bump_revision`、只发一次加载；验证：新增 Rust 测试覆盖「N 个文件恰好 N 个 entry 且从第 1 首播」「库内库外混合顺序」「替换当前队列与随机轮」「队列投影当前项置顶」，运行 `cargo test -p echo-desktop --all-features`。
- [x] 3.2 在 `commands_file_open.rs` 新增 `play_opened_files(paths)`（因 §2.1 规模门禁从 `commands.rs` 拆出文件打开命令模块），按输入顺序解析活动库内 UUID / 库外临时项并调用 3.1 的批量入口；删除 `play_temporary_file`；`bootstrap.rs` 命令表同步；验证：新增边界测试覆盖「库内不建临时项」「库外不复制入库」「资料库只读/不可用时仍可播且未调用写入门」，运行 `cargo test -p echo-desktop --all-features`，并用 `rg -n "play_temporary_file" apps crates` 确认零残留。
- [x] 3.3 更新 `crates/echo-desktop/src/ipc/generate.rs` 与 `apps/desktop/src/bridge/index.ts` 的命令类型，重建生成物 `apps/desktop/src/ipc/ipc-types.generated.ts`；验证：`pnpm --filter @echo/desktop generate:ipc` 后 `git diff --exit-code -- apps/desktop/src/ipc/ipc-types.generated.ts` 无漂移，`pnpm --filter @echo/desktop typecheck` 通过，CI 的 IPC 生成物漂移检查通过。
- [x] 3.4 本组静态检查与回归；验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test -p echo-desktop --all-features` 全部通过。

## 4. 前端：批量订阅与投放反馈层

- [x] 4.1 `App.tsx` 把 `app://file-open-request` 订阅改为对整个 `paths` 数组**一次**调用 `play_opened_files`，保留「已解码绝对路径、禁止二次 URL 解码」的注释与契约；验证：改写 `App.test.tsx` 断言「多路径只触发一次批量命令」「含字面 `%20` 的路径原样传递」「冷启动补发仍播放」，运行 `pnpm --filter @echo/desktop test -- --run App.test.tsx`。
- [x] 4.2 新增投放反馈组件：订阅 `app://file-drop-state`、effect 清理订阅、可读名称、`pointer-events: none`、`prefers-reduced-motion` 去动效但不丢状态变化，不注册进 `useOverlay` / `OverlayTier`；验证：新增组件测试覆盖出现/消失、可读名称、减少动画、与标题栏拖拽条及 Windows 自绘按钮共存，运行 `pnpm --filter @echo/desktop test -- --run`。
- [x] 4.3 在 `docs/prototype/echo-desktop-player.html` 补投放反馈层（复用 #74 的克制浮层材质，不引入第二套浮层样式），重跑 `node scripts/extract-prototype-css.mjs`，只提交生成物；验证：`git diff -- apps/desktop/src/styles/` 与脚本输出一致，且 `pnpm --filter @echo/desktop build` 通过。
- [x] 4.4 本组静态检查与回归；验证：`pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test -- --run`、`pnpm --filter @echo/desktop build` 全部通过。

## 5. 文档、规格与场景登记

- [x] 5.1 `docs/PRODUCT.md` 补「拖入窗口 = 双击打开、只播放不入库」的产品表述，`docs/interface-terminology.md` 在「覆盖层与反馈」补「投放反馈层」、在播放栏/队列相关段落补「临时」标注及不进统计/会话恢复的边界；验证：`rg -n "投放|临时" docs/PRODUCT.md docs/interface-terminology.md` 覆盖 Q1/Q2/Q4 结论，且未出现「拖放即导入」表述。
- [x] 5.2 把三个 delta 的新场景按尾部新 ID 登记到 `docs/traceability.md` 与 `scripts/verify/scenario-commands.mjs`（命令 + 非空 evidence 指向任务组 2/3/4 已落地的测试），重建 `scripts/verify/manifest.json`；验证：`node scripts/verify/reconcile-scenarios.mjs`、`node scripts/verify/validate-scenario-commands.mjs`、`node scripts/verify/gen-scenario-manifests.mjs`、`pnpm verify:governance -- --sections=registry,rust-static` 全部通过，且既有 577 个 ID 未变化。
- [x] 5.3 逐条执行新增场景的验收命令，确认 evidence 目标在输出中被观测到；验证：`pnpm verify:scenario DAS-R26-S01 DAS-R26-S02 DAS-R26-S03 DAS-R26-S04 DAS-R26-S05 DAS-R27-S01 DAS-R27-S02 DAS-R27-S03 DAS-R27-S04 DAS-R27-S05 DAS-R27-S06 DAS-R27-S07 DAS-R28-S01 DAS-R28-S02 DAS-R28-S03 DP-R07-S05 DP-R07-S06 DP-R07-S07 DP-R07-S08 SFI-R07-S06 SFI-R07-S07 SFI-R07-S08 SFI-R07-S09` 全部返回零；ID 由 `node scripts/verify/spec-scenarios.mjs --check` 派生，若归档后顺序有变以该输出为准。

## 6. 集成验证、规格同步与交付

- [x] 6.1 全量后端与前端门禁；验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-targets --all-features`、`pnpm --filter @echo/desktop format:check`、`pnpm --filter @echo/desktop lint`、`pnpm --filter @echo/desktop typecheck`、`pnpm --filter @echo/desktop test -- --run`、`pnpm --dir apps/desktop build`、`pnpm verify:frontend-fresh` 全部通过。
- [ ] 6.2 完整治理与场景门禁；验证：`pnpm verify:governance`、`pnpm verify:scenario --automated`（540 个自动化场景）、`pnpm verify:scenario -- --all`、`openspec validate --strict`、`openspec validate --specs` 与三方场景集合对账。`--automated`、governance、strict 与对账均通过；`--all` 因既有场景 `SFI-R11-S05` 缺少真机 `native-attestation` 记录而失败（该行未在本 change 改动，见交付说明），需 6.3 的真机证据补齐。
- [ ] 6.3 三平台原生冒烟：单文件松手即播、多文件按序成队列、文件夹与非音频可读拒绝、冷启动投放、标题栏拖拽条与 Windows 自绘按钮命中、沉浸模式与浮层下 Escape 逐层退出、资料库只读时仍可播、含中文/空格/`%20` 文件名；验证：按 `docs/native-attestation-playbook.md` 产出三平台证据日志，`artifacts/native-attestations/` 对应条目可被 `check-native-attestation.mjs` 判定通过。
- [ ] 6.4 按 `AGENT.md` 把三个 delta spec 同步到 `openspec/specs/` 并验证同步结果，再归档 change `add-window-file-drop-playback`；验证：`openspec validate --strict` 通过、主规格 diff 与 delta 内容一致、`openspec archive` 成功且 change 不再处于 in-flight 状态。
- [ ] 6.5 在独立分支 `feat/issue-117-window-file-drop-playback` 上创建关联 #117 的中文 Pull Request，描述 Q1/Q2/Q3 决策、批量队列修复与三平台验证结论；验证：`gh pr view` 可见关联 issue、CI 全绿，且未直接向 `main` 推送。
