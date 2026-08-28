# Echo 0.1.0 — 第四阶段实现提示词

> 这份提示词用于交给一个独立 agent 实现 **阶段 4（媒体解析、资料库扫描与监听）**，覆盖 task 4.1–4.10。
> 该 agent 只做 **实现**（apply），不进入 propose/plan。所有规格、设计与验收以 change 为准。

---

## 你（agent）的角色

你是 Echo 项目的实现 agent。Echo 是本地优先的跨平台音乐播放器。你负责把下面标注的 OpenSpec change 中 **阶段 4（tasks 4.1–4.10）** 实现为符合规格的代码和通过验收的可执行验证。

**阶段纪律**：本阶段只做实现。规划已经在 `docs/` 与 change 的 `design.md` 中完成。不要新增未在任务清单中列出的架构改动，不要进入 propose/update 阶段。如果发现规格、设计与现有实现冲突，**先停下来**向调用方报告，不要擅自改规格。

---

## 工作区与流程约定（必须遵守）

- 工作目录：`/Users/xian/Project/music/Echo`
- 这是一个 Rust workspace + pnpm workspace。阅读 **`CLAUDE.md`（项目级）** 了解总体原则与 OpenSpec 纪律。
- **CodeGraph**：仓库已建立 `.codegraph/` 索引。理解/定位已有代码时，优先用 `codegraph explore "<符号名或问题>"` 一次拿回符号源码与调用路径，而不是盲目 grep + 通读文件。返回的源码视同已 Read，可直接用于编辑。
- **应用 OpenSpec apply 流程**：开始实现前调用 `openspec-apply-change` skill 装载本 change 的 apply 上下文，确认当前 change 处于该被实现的任务分组；完成后用 `openspec-sync-specs` 同步规格，并只在全部验收通过后按仓库约定标记完成（不要在本阶段归档；4–13 阶段后续才做）。
- **验收执行器**：任务 1.3 已建立统一执行器。阶段 4 本组验收聚合命令是：

  ```
  pnpm verify:task -- 4.1 4.2 4.3 4.4 4.5 4.6 4.7 4.8 4.9 4.10
  ```

  单项开发时使用相同命令只传该 task ID。命令、fixture、过滤器或人工步骤未登记，或命令未通过，任务不得勾选完成。

---

## 阶段 4 目标与总体设计（来自 change design.md）

阶段 4 把 Core 从「领域模型 + SQLite 存取」推进到「能真正吃进一个目录」：根目录切换屏障、受根边界约束的文件系统 Adapter、媒体解析（probe + lofty 标签/封面/歌词）、输入上限、LRC 解析、封面缓存、generation 扫描流水线、快速跳过/重关联、文件监听与进度持久化。

依赖顺序：Stage 2 已定义全部 port（`MediaProbe`、`MetadataReader`、`ContentHasher`、`CoverCache`、`LyricsParser`、`FileEventSource`、`LibraryFileSystem`、`Clock`、`IdGenerator`），Stage 3 已建立 SQLite Repository/`scan_runs`/`scan_issues`/`cover_assets` 等表与写权限。本阶段实现这些 port 的真实 Adapter 与扫描用例，并把它们与 Stage 3 的 SQLite 层接起来。

阶段 4 依赖 `crates/echo-core/src/domain/state/scan.rs`（扫描状态机）、`crates/echo-core/src/domain/state/library_root.rs`（`RootEpoch`、`RootSwitchBarrier`、`LibraryRootState`）与 `crates/echo-core/src/application/ports.rs`（全部 Port 声明）。`application/testing/*` 下有可复用的内存/脚本替身（`ScriptedFileEvents`、确定性 `MediaProbe` 等），先读它们再决定新的测试替身放哪。

新增 infrastructure 模块（`infrastructure/mod.rs` 目前只有 `sqlite`）：

- `crates/echo-core/src/infrastructure/filesystem/`（阶段 4 主体：根受约束 Adapter、受控暂存目录、扫描 walker、监听）
- `crates/echo-core/src/infrastructure/metadata/`（lofty、probe、封面缓存、LRC parser）

`infrastructure` 层不得定义业务规则，只实现 `application::ports` 声明的接口。保持 `echo-core` 不依赖 Tauri/mpv/React。

---

## 关键设计决策（实现必须遵循）

### A. 根目录切换屏障（task 4.1）
根切换由桌面 runtime 以 `Prepare → QuiesceOldRoot → CommitActivation → RebindRuntime` 执行，用 `root_epoch` 拒绝旧任务迟到结果（见 `RootEpoch`）。屏障覆盖播放中/扫描中/导入中/删除撤销中切根。候选成功判据：目录完整枚举、未取消、reconcile 成功、无根级错误且单文件错误已形成 summary；空目录允许成功，枚举中断或根级错误不得激活；重选同一规范路径复用原 `LibraryRootId`。只读根 `ActiveReadOnly`：可扫描/搜索/播放，禁用导入/删除等写操作。根失败只改变 availability，不清除记录。

### B. 受根边界约束的文件系统 Adapter（task 4.2）
`LibraryFileSystem` 的 Adapter 内部解析根的绝对路径；用例只传 `RelativeMediaPath`。枚举不跟随目录 symlink，候选 canonicalize 后必须仍在根内（防循环/逃逸）。受控暂存目录：exclusive-create + 随机名（如 `.echo-staging-<128-bit-random>`）+ 写入含 application magic、`LibraryRootId`、格式版本的所有权 marker；只有 marker 完全匹配的目录才可写/清理/被扫描忽略。同名已存在、marker 无效、目录是 symlink/reparse point 时另择随机名，无法安全建立则禁用写能力，绝不接管或忽略用户目录。`write_capable` 反映权限 + marker。

### C. 媒体解析（task 4.3、4.4）
- `MediaProbe`：独立于标签读取，探测容器格式与是否有音轨、时长、音频参数。支持矩阵按 spec：MP3、FLAC、M4A、经内容探测确认有音轨的 MP4、Ogg Vorbis、Opus、WAV。伪装扩展名、无音轨 MP4、损坏文件形成单文件诊断（`Unsupported`/`NoAudioTrack`/失败）。扩展名只做廉价候选过滤，随后 probe。
- `MetadataReader`：lofty 解析标签/封面/歌词；标签字符串做 Unicode NFKC、控制字符清理、展示兜底；时长/格式从流探测，不轻信标签；原始文件不修改。
- 输入上限（task 4.4）：标签 4 KiB、歌词候选 2 MiB、封面 20 MiB；超限资产被安全跳过，歌曲其余字段仍可入库。

### D. LRC parser 与候选选择（task 4.5）
优先级：覆盖层 > 有效内嵌 > 有效侧车（同名 `.lrc`，扩展名大小写不敏感）；高优先级损坏时回退下一有效来源。LRC parser 保留原始文本并生成排序后 `{timestamp_ms, text, original_index}`；无有效时间戳则标为纯文本。损坏/空歌词不阻断入库，诊断与来源单独记录。任务验证：乱序/越界时间戳、纯文本、空歌词、损坏高优先级回退。

### E. 封面缓存（task 4.6）
按内容 hash 存应用缓存目录，生成列表/详情两个有尺寸上限缩略图；`CoverCache` 返回不透明 asset key，绝不给 UI 绝对路径。React 通过只读、严格 key 校验的自定义 asset protocol 获取。缓存 key 与 DB 引用事务一致；后台 GC 按容量上限清理，且不删除仍被引用资产（`gc(&referenced_keys)` 保留引用集）。大图不进歌曲列表查询。

### F. generation 扫描流水线（task 4.7、4.8、4.10）
- 状态机 `Queued → Enumerating → Parsing/Hashing → Reconciling → Completed / Cancelling → Cancelled / Failed`（对应 `domain/state/scan.rs`）。
- 结果按小批次 reconcile（每批不超过配置记录数/时间预算），扫描期间 UI 查询必须可用；写事务小批量，不跨 `.await` 持锁。
- 有界 worker：hash/lofty/歌词/缩略图用有界 `spawn_blocking`/专用 rayon 池，默认并发 ≤ `min(CPU,4)`；每扫带 `generation_id`；只有完整扫完成后才把「上一 generation 出现、本 generation 未出现」的歌曲标 missing——**取消/枚举失败不得批量误删**。
- 快速跳过（task 4.8）：size/mtime 未变且已有 hash/解析结果则跳过重算。重关联：相对路径未变优先复用 UUID；旧路径缺失且 hash 唯一相同则重关联；首次多同 hash 路径以规范路径键最小者为主路径，其余记 issue，主路径消失后按同规则提升；hash 不同时仅当「唯一缺失候选 + 唯一新文件 + 标准化音乐键相同 + 时长误差 ≤2s」才做音乐键弱重关联，任何歧义保留旧 missing 并新建 UUID，不做错误合并。
- 进度持久化（task 4.10）：progress/summary/issue 写 `scan_runs`/`scan_issues`；进度节流不高于 100 ms 一次；终态不丢失；坏文件不阻断其他歌曲。支持取消与手动重扫用例。

### G. 文件监听（task 4.9）
- 事件按规范路径去重并防抖；创建/修改等 size/mtime 连续两次稳定再解析（双采样）。
- watcher 在 reconcile 前查询活动 target claim：命中导入 operation 路径→延迟处理；若文件已完整发布则复用 journal 预留 `SongId`，不得创建第二 UUID。
- 全扫期间事件缓存；overflow、根替换或无法归类的 rename 降级为增量/全量重扫。
- 注入新增/修改/删除/rename 的乱序、重复、丢失及 publish 后 DB commit 前抢先事件，最终状态收敛且 UUID 等于 journal 预留值。

---

## 交付物（DoD）

1. 新增/完成的代码如下：
   - `infrastructure/filesystem/`：根受约束 Adapter（实现 `LibraryFileSystem`）、受控暂存目录与 marker 校验、扫描 walker、BLAKE3 hash（实现 `ContentHasher`）、文件监听 Adapter（实现 `FileEventSource`）。
   - `infrastructure/metadata/`：`MediaProbe` 实现（容器/音轨/时长/音频参数）、`MetadataReader`（lofty 标签/封面/歌词）、封面缓存（实现 `CoverCache`）、LRC parser（实现 `LyricsParser`）。
   - 应用层：根切换屏障用例（`PrepareLibraryCandidate`/`ActivateLibrary` 相关）、`StartScan`/`CancelScan`/`ReconcileFsChanges`/`RelinkLibrary` 用例实现，串联 SQLite Repository 与上述 Adapter。
2. 每个 task 4.1–4.10 都有对应单元/集成测试，覆盖 task 文本列出的验证点（见 tasks.md 逐条）。测试不得访问真实用户目录；文件/监听从 `application/testing` 替身或临时目录注入。
3. 全部 10 个 task 的验收命令通过：`pnpm verify:task -- 4.1 … 4.10`（或逐 task）。
4. 更新 `fixtures/audio` 来源清单/checksum（如新增样本）；新增的 fixture 必须许可清晰。
5. 无 clippy warning、`cargo fmt --check` 通过、`cargo test -p echo-core --all-features` 通过；不回归阶段 1–3 验收。
6. 用 `openspec-sync-specs` 同步规格，并更新 traceability（如任务清单已映射场景）。

---

## 执行建议（可并行 / 可复用）

- 先读 `application/ports.rs`（全部 Port 签名）、`domain/state/scan.rs` + `domain/state/library_root.rs`、`application/testing/`（现成替身）、`infrastructure/sqlite/`（Repository 与 `scan_runs`/`scan_issues`/`cover_assets` 存取）、`docs/DESIGN.md` 与 `design.md` 中第 5–7 节。
- 建议依赖顺序：4.2（fs Adapter + marker）→ 4.3/4.4（probe + lofty + 上限）→ 4.5（LRC）→ 4.6（封面）→ 4.1（切根屏障，依赖 adapter）→ 4.7/4.8（扫描流水线）→ 4.9（watcher）→ 4.10（进度）。4.1 涉及 runtime 装配，如 desktop runtime 尚未就位，以 Core 用例 + 状态机测试为先，把 runtime 装配点留清晰接口。
- 标有需求标签的 task（`[local-library]`、`[desktop-app-shell]`、`[safe-file-ingestion]`、`[desktop-playback]`、`[immersive-lyrics]`）表明其规格来自对应 spec；实现与验收须以该 spec 的可观察行为为准（见 `specs/<tag>/spec.md`，尤其 `local-library` 的格式矩阵、歌词/覆盖层优先级、搜索/排序）。

---

## 工作完成判定

全部 task 的 `pnpm verify:task` 命令通过 + 全量质量命令（`cargo fmt/clippy/test`）通过 + 规格同步完成，才算本阶段完成。任何一个 task 未通过验收或存在规格变更诉求，必须在交付摘要里如实说明，不得以人工描述替代 manifest 命令。
