## Why

在 Finder 里双击音频文件（已把 Echo 设为默认打开方式）后**没有声音、进度不走**，且标题显示为 `We%20Will%20Rock%20Yo…%20Queen.flac`（GitHub issue #6）。根因是 macOS 的文件关联事件下发的是 `file://…` **URL 字符串**，而 `main.rs` 把它原样当路径下发（`deliver_file_open(app, url.to_string())`）；全链路没有一层把 URL 归一化成文件系统路径，最终被 actor 的安全边界（`is_local_media_path` 拒绝含 `://` 的路径）正确拦下，于是 `PlaybackState::Failed` 且**不向 mpv 投递任何 load**。

这不是「某个函数写错了」，而是**两个正确行为之间无人负责的缝隙**：`is_local_media_path` 的拒绝是对的（task 8.3 纵深防御），`App.tsx` 从路径末段取显示名也是对的；错的是**没有任何一层承担 OS 边界上的 URL→路径转换责任**，而这份责任只写在注释里（`main.rs:461-465` 的 "the payload is a single absolute path"）。

更值得注意的是一条 issue 未覆盖的复验结论：**该缺陷所在的需求，恰好是门禁最薄的地方**。

| # | 门禁层面的事实（HEAD `e48ae6e`） | 证据 |
|---|---|---|
| G1 | `DAS-R06-S01/S02/S03`（单实例与文件打开唤醒）与 `SFI-R07-S01/S02`（单实例唤醒与重复打开）这 5 条场景的验收命令**全部**是 `task-9.1.mjs`，而该检查只做 `cargo check/clippy -p echo-app` + 3 个 FIFO 单测 + `main.rs` 的 token 存在性 grep + `app.emit(FILE_OPEN_REQUEST` 计数 ≤2 | `node -e "…scenarioCommands()"` 逐条列出；`scripts/verify/checks/task-9.1.mjs:38-86` |
| G2 | `url.to_string()` 与 `path.to_string_lossy()` 对 G1 的任何一条断言**都无影响** ⇒ 门禁对「URL 未归一化」恒绿 | 同上：6 个 token 与 1 个计数都不随载荷形态变化 |
| G3 | `SFI-R06-S01/S02/S03`（外部文件直接打开 / 活动资料库内文件 / 非活动旧库文件）的命令是 `task-12.7.mjs`（安全加固：路径穿越、TOCTOU、恶意标签、asset key、capability），与三条场景的 THEN 子句**无交集** | `grep -in "temporary\|file-open\|file_open\|fileopen\|uuid\|Opened" scripts/verify/checks/task-12.7.mjs` → **命中 0** |
| G4 | 注入证明的完整性规则是**二分类**的：只要检查里出现 `spawnSync`/`execFileSync` 就被判为「delegating」而**整体**豁免注入证明——即使它还在自身内部做结构性断言。`task-9.1.mjs` 正是这种混合形态（1 处 `spawnSync` + 6 个 token `includes` + 1 个计数比较） | `scripts/verify/injection-suite.mjs:864-912`（`delegating` 判定在 `:910`） |
| G5 | 87 个已登记检查中，12 个有注入条目、**75 个落入豁免桶、`UNPROVEN` 恒为 0** ⇒ 「自包含检查必须有注入证明」这条规则当下**未约束任何检查** | 复现脚本见 `design.md` 的 E18 |
| G6 | `tests/native/` 下 50 个 `.md` **全部**仍是未填写模板，`artifacts/native-attestations/` 为空目录 ⇒ 走人工举证的 35 条场景从未被执行 | `grep -rl "填写该场景的具体可执行步骤" tests/native/ \| wc -l` → 50 |

因此本 change 的目标是**双重的**：修好边界（让双击真的出声），并让门禁能证伪这条边界（否则下一次同样的缝隙还会静默通过）。后者比前者更重要——`is_local_media_path` 已经正确工作了，真正失职的是验收链路。

## What Changes

- **OS 边界归一化（主修）**：`RunEvent::Opened` 分支 MUST 用 `url::Url::to_file_path()` 完成 scheme 校验与百分号解码，再进入启动 FIFO；非 `file` scheme（`http://`、`smb://`…）MUST 被丢弃并记录告警，MUST NOT 以任何形态进入播放链路。**BREAKING（内部 API）**：`deliver_file_open` 与 `StartupSupervisor::receive_file_open` 的入参从 `String` 收窄为 `PathBuf`，使「把 URL 原样当路径下发」**无法编译通过**——契约由类型承载，而不是靠注释。
- **前端不做二次解码**：`App.tsx` 收到的载荷已是解码后的路径。文件名可以**合法地**包含 `%20` 字面量，因此前端 MUST NOT 调用任何 URL 解码；补一条用例固定这个「不做」的契约，防止修复时两头都改造成双重解码。
- **进度事实在加载开始时重置**：actor 在每次开始加载（库内歌曲、暂停加载、临时项三条路径）时 MUST 先清空快照的 `position`/`duration`。当前三个 `Failed` 产生点都不清，于是「无法播放」时界面仍显示**上一次成功加载文件的时长**（实测截图 `5:18`），属于诊断噪音。**BREAKING（快照语义）**：失败/加载中的快照现在是不带旧进度的。
- **保留纵深防御**：actor 对含 `://` 路径的拒绝 MUST 保持不变。本 change 的修复点在其**上游**，不得为了「能播放」而放宽这一层。
- **门禁通电**：扩展 `task-9.1.mjs`，让它对归一化做**行为级**断言（`open_targets` 纯函数单测 + 断言 `RunEvent::Opened` 分支不再出现 `url.to_string()`），并为它补一条注入证明条目；把 `SFI-R06-S01/S02/S03` 从 `task-12.7.mjs` 这个无关检查迁到能证伪其 THEN 子句的命令上。全部改动经 `scripts/verify/scenario-commands.mjs`（唯一权威源）→ `gen-scenario-manifests.mjs --write` 重生成派生清单。
- **治理约束**：新增一条可机械校验的要求——作为场景验收命令的自包含检查 MUST 在注入证明套件里有条目，且**委托子进程的检查不得因此豁免它自身内部的断言**。G3/G5 表明这里有两种不同形态的假绿：「登记了但断言与场景无关」与「靠 delegating 分类整体豁免」。G5 的数字说明第二条尤其危险：豁免桶占 75/87，`UNPROVEN` 恒为 0，规则看起来一直在生效，实际上一条检查也没约束到。

不新增 IPC 命令、不改 `main.json` 权限集、不引入新依赖（`url::Url::to_file_path` 已在 `Cargo.lock` 中随 tauri 传递存在）。

## Capabilities

### New Capabilities

无。本 change 只修正既有能力与既有实现之间的差异，并给已有能力补上它缺失的边界契约。

### Modified Capabilities

- `safe-file-ingestion`：新增 `操作系统文件打开载荷必须先归一化为本地路径`（既有 `源文件、系统关联与安全边界` 只声明了「打开即播放」的结果，没有任何一条断言载荷形态；这正是缝隙的位置）。既有需求文本与场景不变，归属保持。
- `desktop-playback`：新增 `加载尝试必须重置进度事实`（既有 `播放错误处理` 只管「失败要提示并跳过」，不管失败快照里残留了什么）。
- `engineering-governance`：新增 `场景验收命令必须自带可执行的失败证明`。

## Impact

- **Tauri 壳**：`apps/desktop/src-tauri/src/main.rs`（`RunEvent::Opened` 分支改为 `to_file_path()`；新增可测纯函数 `open_targets(&[Url]) -> Vec<PathBuf>`；`deliver_file_open` 与 `record_gate_open` 的入参类型随之收窄）。**不改** `invoke_handler` 的命令集，**不改**权限，因此 `main.json` / `security.rs` / `gen/schemas/` 不需要联动。
- **Desktop 平台层**：`crates/echo-desktop/src/runtime/mod.rs`（`StartupSupervisor::receive_file_open` 与 `pending_opens` 队列的元素类型 `String` → `PathBuf`；`tests/native` 无关，但 `runtime::tests` 的 3 个用例载荷需同步改类型）。
- **Desktop 播放层**：`crates/echo-desktop/src/player/actor.rs`（`load_path` 与两个 `resolver` 失败分支统一为「开始一次加载」入口并清空 `position`/`duration`/`pending_seek`）。`is_local_media_path` 与其测试**不动**。
- **前端**：`apps/desktop/src/app/App.tsx`（注释澄清载荷形态；逻辑不改）+ `App.test.tsx`（新增「前端不解码」用例；既有 `file://` 形态用例保留为回归）。**不改** `apps/desktop/src-tauri/src/commands.rs` 的 `play_temporary_file(path: String)`：WebView 边界无法承载 `PathBuf`，这一点由前端用例与壳侧类型共同兜住。
- **门禁与追溯**：`scripts/verify/checks/task-9.1.mjs`（新增第 4 项验收）、`scripts/verify/injection-suite.mjs`（新增 `file-open-normalization` 条目）、`scripts/verify/scenario-commands.mjs`（唯一权威源：`SFI-R06-S01/S02/S03` 换命令）→ `gen-scenario-manifests.mjs --write` 重生成 `manifest.json.scenarios[]` / `tests/scenarios/*.yaml`；`docs/traceability.md` 与 `docs/native-attestation-playbook.md` 中相关行的覆盖描述更正。
- **可复现的验证方式（仓库已有设施，不需新工具）**：`main.rs:449` 的 `record_gate_open` 已支持用 `ECHO_GATE_OPEN_LOG` 把每条实际下发的路径落到日志。修复前后各跑一次即可对比形态：

  ```bash
  ECHO_GATE_OPEN_LOG=/tmp/echo-opens.log <启动 Echo>
  # 在 Finder 里双击文件名含空格/中文的音频
  cat /tmp/echo-opens.log
  # 修复前：file:///Users/…/We%20Will%20Rock%20You%20-%20Queen.flac
  # 修复后：/Users/…/We Will Rock You - Queen.flac
  ```

- **兼容性**：`FILE_OPEN_REQUEST` 事件的载荷形状（字符串数组）不变，前端与桥接无需改；`play_temporary_file` 的 IPC 形状不变。因此**没有对外兼容性破坏**，破坏面全部落在 crate 内部的 Rust 签名上。
- **平台面**：缺陷仅 macOS（`RunEvent::Opened` 由 tauri 的 `#[cfg(any(target_os = "macos", …))]` 隔离）。Windows/Linux 的 argv 支路传的是平台原生路径，不受影响。

## 非目标（本 change 不做）

- **不接线 `on_active_root`，不做 UI 调整**。issue 的附节 1–3（沉浸页去掉 `临时` 徽标 + 导入按钮、底部播放条改小号「导入」、文件已在资料库目录时不显示导入）以及它们的前置条件 `TemporaryItem.on_active_root`（`commands.rs:656` 硬编码 `false`，全仓无读取点，唯一非构造引用是 `coordinator.rs:263` 的字段搬运），按 issue 自己的建议拆到独立 issue。理由：那是交互层决策，与本 bug 的根因无关；把它塞进来会让本 change 的验收从「双击能出声」漂移成「导入入口的视觉规范」。
- **不引入 `MediaPath` newtype**。把 shell 与 runtime 之间的载荷收窄为 `PathBuf` 已经实现了「让非法形态无法编译」的目的；newtype 只会与 `TemporaryItem.path: PathBuf` 产生第二套类型，留到独立 issue 再评估。
- **不放宽 `is_local_media_path`**，也不改 `protocol-whitelist=file`。
- **不填 `tests/native/` 的 50 个空模板**（G6），也不新增针对「场景命令靠兜底降级为人工举证」的检查——这是仓库级的既有状态，影响面远超本 bug，需要独立 change 单独评估（在 design 的「范围决策」中记录）。
