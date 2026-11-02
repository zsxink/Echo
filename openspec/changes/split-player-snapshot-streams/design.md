## Context

本 change 处理的是**同一条 10 Hz 传输链路上的三项独立开销**，它们共享同一个根因结构：*与内容无关的高频重复工作*。

### 实测复核（对 issue #112 的重新界定）

采样条件：v0.3.1 release，主进程 pid 2109，480 首曲库，前台播放中，模式为随机播放，EQ 预设为非中性的 `builtin:vocal`。

| 观测 | 数值 | 判读 |
|---|---|---|
| `footprint` | 首次 522 MB，`phys_footprint_peak` 650.8 MB，谷底约 200 MB | 大幅摆动 |
| `heap` | 125,172 节点 / 22.1 MB 活跃 | 大部分是已释放但仍计费的页 → 不是泄漏 |
| `Malloc Small` | 59.5 MB 总量 / 43.4 MB 常驻，全程稳定 | 两次队列克隆落在这里，**不是**主因 |
| `Malloc Large` | 每 2 秒涨约 19 MB、掉 50–80 MB；稳态 90 MB → 175 MB（约 20 秒一周期） | **≈4.25 MB/s 大块 churn** |
| `vmmap` | 1618 块**恰好 304 KB** 的匿名块，合计 492 MB | 同一结构体的重复拷贝 |

`304 KB` 这个尺寸是关键线索：10 Hz 载荷实测 148,058 字节（480 条队列，均摊 308.5 B/条）加约 10 KB 音效 = 约 163 KB。Tauri 事件链把它复制三次——`serde_json::to_string`（148 KB）、`emit_js_script` 的 `format!`（148 KB）、`NSString::from_str` 的 UTF-16（296 KB，页对齐后 304 KB）。三次都 ≥128 KB，因此全部落进 macOS malloc 的 **large zone**，而已释放的页在内核回收前仍计在 footprint 上。量化核对：3 × 148 KB × 10 Hz ≈ **4.4 MB/s**，实测 **4.25 MB/s**，吻合。

结论：**载荷体积越过 128 KB 大块分配阈值**才是内存主因；#112 归因的「每帧克隆整队两次」是小对象 churn，实测稳定在 42 MB 不变，量级差两个数量级。

### 同一链路上的 CPU 主因

`sample` 抓取的 `echo-snapshot-forwarder` 线程调用图：220 个采样中 **203 个**落在 `EffectsState::snapshot → math::analyze → peak_gain → response_db → log10`。`peak_gain` 用 `INTERVALS = 32768` 做等间隔扫频，每频点对 10 个 biquad 各算一次 `log10`（单次约 327,680 次 `log10`），另加每个局部极大值的 `refine_peak`（48 次三分搜索 × 2 次求值）。EQ 非中性时这条链每秒跑 10 次，与队列长度无关。

### 队列投影的复杂度

`Queue::view_entries()` 用 `Vec::contains` 判重、线性 `get()` 查条目，随机模式下还对 `pending` 与 `shuffle_bag` 各做一次线性查找 → 每帧 O(n²)。480 条时约 23 万次 UUID 比较。

### 当前实现核对

| 位置 | 现状 |
|---|---|
| `crates/echo-desktop/src/runtime/player/mod.rs:134` | `UiPlayerSnapshot.queue: Vec<UiQueueEntry>`——注释即写明「The full queue the panel renders」 |
| `crates/echo-desktop/src/runtime/player/snapshot.rs:29-82` | `build_queue` 每帧克隆每条的 `entry_id`/`song_id`/`title`/`artist`/`cover_key` |
| `crates/echo-desktop/src/runtime/player/snapshot.rs:216-221` | 转发线程每帧 `queue_provider()` + `metadata.resolve(&view.entries)` |
| `apps/desktop/src-tauri/src/main.rs:559,562` | `queue_provider` 每帧调 **两次** `coord.queue_view()` |
| `apps/desktop/src-tauri/src/main.rs:571-574` | 发射闭包里 `effects.snapshot().snapshot().into()` 每帧重算音效快照 |
| `crates/echo-desktop/src/player/queue.rs:377-408` | `view_entries()` 为 O(n²) |
| `crates/echo-desktop/src/player/queue.rs:133-161` | `Queue` **没有任何修订号字段** |
| `crates/echo-desktop/src/player/coordinator.rs:40-57` | 协调器持有 `queue` / `mode` / `failed_round`，**没有**修订号 |
| `crates/echo-desktop/src/effects_service/mod.rs:80-86` | `capture()` 每次调用都 `saturating_add(1)` → `snapshot_sequence` 是**捕获计数**，不是变更计数 |
| `apps/desktop/src/player/playerStore.ts:103-110` | `publish()` 整体替换快照对象 → 每帧新的队列数组标识 → 面板每帧重渲染 |
| `apps/desktop/src/features/player/QueuePanel.tsx:124-161` | `entries.map(...)` 为**每个**队列条目渲染一行 `<button>`，无窗口化 |

## Goals / Non-Goals

**Goals**

- 10 Hz 位置流的载荷降到 KB 量级，且 MUST NOT 携带队列条目与音效快照。
- 队列投影与音效快照各自按「内容确实变化」下发，变化判定必须是 O(1) 或 O(内容) 且不重复计算昂贵派生量。
- 队列投影构造为 O(n)。
- 队列面板的 DOM 行数与队列长度解耦。
- 队列语义、音效计算结果、持久化与恢复语义**一字不动**。

**Non-Goals**

- 不做队列条数上限截断（产品行为变更）。
- 不做拖动重排——#112 的验收项提到它，但代码库里**不存在**该功能（没有对应 mutator、IPC command 或 UI）。
- 不裁剪 `UiQueueEntry` 的字段：本次的根因是「整队重复传输」，不是「单条太胖」。
- 不改音效的数值语义（扫频精度、预放余量、频响曲线），音质相关讨论见 #109。
- 不做 5 万首曲库的分页化（见 #114）。

## Decisions

### 1. 拆成三条独立事件，而不是「一个事件 + 可选字段」

`player://snapshot`（10 Hz，传输状态）、`player://queue`（队列修订号变化时）、`player://effects`（音效快照序号变化时）。

可选字段方案（同一个事件，队列与音效字段时有时无）的问题在于：事件语义随载荷形状漂移，前端必须按字段存在与否分支，后端也无法对高频流断言「载荷有上限」。三条流各有独立的变更判据，前端可以分别判定新鲜度，高频流的体积上限可以用一条可测的断言锁死。

**载荷与事件名**：

| 事件 | 频率 | 载荷 |
|---|---|---|
| `player://snapshot` | 10 Hz | `UiPlayerSnapshot`（去掉 `queue` 与 `effects`，新增 `queueRevision`） |
| `player://queue` | 队列修订号变化时 | `{ revision: u64, entries: UiQueueEntry[] }` |
| `player://effects` | 音效快照序号变化时 | `EffectsSnapshotDto`（字段不变，`snapshotSequence` 语义改为内容变更计数） |

`UiQueueEntry` 的字段**保持不变**（`entryId`/`songId`/`title`/`artist`/`durationS`/`coverKey`/`flags`），本 change 只改下发频率。

### 2. 队列修订号由 `Queue` 持有，其 mutator 自增

新增 `QueueRevision(u64)`（新类型，参照 `crates/echo-desktop/src/ipc/events.rs` 里 `CatalogRevision` / `EventSequence` 的写法）。`Queue` 增加 `revision` 字段，所有会改变投影的 mutator 自增：`push` / `insert_next` / `promote_to_priority` / `remove` / `remove_song` / `remove_direct` / `set_current` / `advance_next` / `advance_in_mode` / `advance_priority` / `previous` / `previous_in_loop` / `clear_pending` / `set_blocked` / `set_priority` / `set_shuffle` / `refresh_shuffle_bag` / `restore_history`。

**为什么由 `Queue` 而不是协调器持有**：`Queue` 的字段是私有的，唯一能绕过它改状态的出口是 `PlaybackCoordinator::queue_mut()`（当前只有删除路径 `runtime/player/coordinated_delete.rs:151` 在用）。修订号放在 `Queue` 内部，任何经由 `queue_mut()` 的改动也会自动推进，无需逐个调用点补漏。

**允许无效调用也推进修订号**：宁可多发一次队列事件，不可漏发。修订号只需要单调、不需要连续。

**协调器额外推进的两处状态**：`failed_round` 的增删（协调器自己持有，不在 `Queue` 内）与播放模式。模式切换多数经由 `Queue::set_shuffle` 已经推进过；重复推进无害。

**队列整体替换时保持单调**：`play_context` / `play_context_paused` / `restore_session` / `play_temporary` 会用新 `Queue` 替换旧的，修订号会归零。统一走一个私有辅助函数 `replace_queue(&mut self, queue: Queue)`：先取出旧修订号，替换后写回，使修订号在一次应用运行期间**只增不减**。`prime_context_paused` 自身不替换队列（它委托给 `play_context_paused`），但会改写播放模式，由模式变更一并推进。

### 3. 修订号与投影必须在同一次加锁内读取

当前 `queue_provider` 闭包整体持有 `coordinator.lock()`。拆分后若分成两次加锁（先读修订号、再取投影），两次之间发生的队列变更会造成「修订号 N 配 N+1 的条目」的撕裂读。

因此队列投影的构造由**一次加锁内完成**：在锁内读修订号；若与上次下发的修订号相同，直接返回「无变化」；否则在**同一次持锁期间**构造投影并返回。修订号与条目因此必然同源。位置流所需的当前条目与模式是 O(1) 读取，与队列修订号分开取，不触发投影构造。

### 4. `view_entries()` 用调用内局部索引改成 O(n)

现状：`emitted` 是 `Vec`（线性 `contains`）、`self.get(id)` 是 `entries.iter().find()`、`pending.contains(id)` 与 `shuffle_bag.contains(id)` 都是 `Vec` 线性查找。

改法：在**一次调用内**构建局部索引——`HashMap<QueueEntryId, &QueueEntry>` 供条目查找，`HashSet<QueueEntryId>` 供 `pending` / `shuffle_bag` / `emitted` 的成员判定。键用拥有型 `QueueEntryId`（12 字节，`Copy`）而非引用：既避免借用检查上的纠缠，复制成本也可忽略。

**为什么用局部索引而不是常驻索引**：`Queue` 有近二十个 mutator，任何常驻索引都要在每个 mutator 里同步维护，漏改一处就产生静默不一致。局部索引每次调用重建一次（480 条约几十微秒），把不变量限制在一个函数内部。

### 5. 音效快照序号改为「内容变更计数」，派生的频响分析按输入记忆化

两件事必须同时做，缺一不可：

**(a) 序号语义修正。** `EffectsService::capture()` 当前每次被调用都 `saturating_add(1)`，因此 `snapshot_sequence` 在 10 Hz 轮询下每帧都在变，**不能**直接当「音效已变更」的判据。改为：捕获后与上一次捕获的状态比较（`EffectsState` 已 derive `PartialEq`；比较时忽略序号字段本身），内容不同才推进序号并记为「已发布」。这样 `snapshot_sequence` 变成真正的变更计数，前端现有的按序号排序逻辑（`playerStore.ts:117-165`）无需改动即可继续工作。

**(b) 派生分析记忆化。** 事件层拆分只能省掉 IPC 拷贝，省不掉 `math::analyze` 的 327,680 次 `log10`。派生的频响分析（`analysis` / `presets` / `response_points` / `editable_bands`）必须按其输入记忆化：键为 `(音效文档, 处理环境)`，值为派生结果。`EffectsDocument` 与 `ProcessingEnvironment` 都是 `PartialEq`，可直接做键。

记忆化放在 `effects` 模块内（`EffectsState::snapshot()` 的派生路径），不放在 IPC 层——`ipc/effects.rs` 只保留 `EffectsSnapshot → EffectsSnapshotDto` 的转换，层级方向不被破坏（见 §3）。

### 6. 当前条目的展示元数据只查一次

`metadata.resolve(&view.entries)` 每帧为整队构建一张 `HashMap` 并克隆每条的元数据。拆分后：位置流只需要**当前条目**的元数据 → 走单条缓存查询；整队元数据解析只在队列修订号变化时随投影一起做。

### 7. 队列面板只渲染可视行

现有规格只约束了面板的**可视高度**（最多 8 行），但实现是 `entries.map(...)` 为每个条目渲染一行 `<button>` + `<QueueCover>`。改为窗口化渲染：按既有行高基准与滚动偏移计算可视区间，只渲染可视行加有限前后余量，上下用占位撑出总高度；队列总条目数仍然显示。

### 8. 启动期与漂移恢复走 `player_sync` 命令

Tauri 事件只在有监听者时才投递。前端注册监听晚于后端启动时，首帧队列事件会丢失；漂移（序号不连续）也需要一个恢复入口。

做法：新增 `player_sync` 命令（无参数）。前端在**注册完三个事件监听之后**调用一次，后端在命令处理里重新下发当前队列投影与当前音效快照。前端收到后按修订号/序号比较：更大才应用，更小或相等则忽略。序号比较是幂等的，因此「命令响应」与「事件」先后到达都不会导致状态错误。

命令只触发下发、不返回大载荷，避免启动期再产生一次 148 KB 的命令响应。

## 兼容与迁移

**IPC 契约变化（事件）**：

| 契约 | 变化 | 兼容影响 |
|---|---|---|
| `player://snapshot` | 载荷去掉 `queue`、`effects`，新增 `queueRevision` | **破坏性**。旧前端会读到 `queue` 为 `undefined` |
| `player://queue` | 新增事件 | 旧前端不监听即忽略 |
| `player://effects` | 新增事件 | 旧前端不监听即忽略 |

`player://snapshot` 的破坏性变化只影响**同仓库内前后端的版本一致性**，不涉及跨版本混用：桌面端前端与后端由同一个安装包一起发布，不存在旧前端连新后端的组合。开发期若前端未同步更新，队列面板会显示为空——因此任务的顺序必须保证前端与后端同批改完，并在 tasks 中把「前端先订阅三个事件」列为前置。

**IPC 契约变化（命令）**：新增 `player_sync`（无参数，返回 void）。只增不改，无兼容影响。

**生成代码**：`apps/desktop/src/ipc/ipc-types.generated.ts` 由 `pnpm --filter desktop generate:ipc` 重新生成，**不得手工修改**（CODE_STANDARDS §2）。

**规格 ID 位移**：`desktop-playback` 新增 4 条 Requirement、`audio-effects` 新增 1 条，会使这两份主规格中后续 Requirement 的场景 ID 顺延。场景登记（`docs/traceability.md`、`scripts/verify/manifest.json`、`scripts/verify/scenario-commands.mjs`）在实现完成、验证通过、**归档时**统一重生成；propose 阶段不动主规格与登记文件。

**行为兼容**：队列语义（顺序、身份、当前项判定、优先区、乱序轮次）、音效计算结果、会话恢复与持久化**均不变**。`player_sync` 只补发当前状态，不改变任何状态。

## 分层与规范核对

| 规范条款 | 本 change 的落点 |
|---|---|
| §3 架构与依赖方向 | 变更全在 `echo-desktop`（Rust 平台层）与 React 表现层；`echo-core` **零改动**。`ipc/effects.rs` 仍只做 DTO 转换，不反向依赖 `effects_service` |
| §2.1 模块与文件组织 | 新增逻辑落在既有文件内（`player/queue.rs`、`player/coordinator.rs`、`runtime/player/snapshot.rs`、`runtime/player/mod.rs`）；不新增巨型文件 |
| §4.1 格式与静态检查 | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| §4.2 错误处理 | 修订号比较、投影构造失败等路径不得 panic；沿用现有 `expect` 仅限已由类型证明不可失败的锁获取 |
| §6 React 与 TypeScript | `playerStore` 新增队列槽位与 `usePlayerQueue()`；`QueuePanel` 保持单一职责（展示与滚动编排） |
| §8 测试与交付门槛 | 每个实现任务给出可执行的验证命令；跨边界契约（事件载荷、命令）补序列化与集成测试 |
| §9 OpenSpec 对代码变更的要求 | 本 change 涉及 Tauri command/event 变化 → 已在本 design 的「兼容与迁移」记录兼容方案；设计模式见 §3.2（Command / Event、State Machine 的既有分层） |

**设计模式**：沿用既有分层，不新增抽象。变更按 **Command / Event** 模式落位——command 表达请求（`player_sync`），event 表达已发生的状态变化（三条流）；队列修订号与音效序号是既有状态机的**版本事实**，不引入新的万能 Service。

## 验证方法

**功能验证**：仓库既有的场景机制（`scripts/verify/manifest.json` + `scenario-commands.mjs`）在归档阶段登记。

**性能验证（本次的核心证据）**：固定条件复测——同一曲库规模（本机 480 首）、前台态、同一播放状态、相同采样时长与间隔，同时记录 `footprint` 总量与 `Malloc Large` 分量。

- 判定：10 Hz 位置流不再产生 ≥128 KB 的大块分配 → `Malloc Large` 分量在播放态不再出现 50–80 MB 的周期性回落，`footprint` 摆幅显著收窄。
- 判定：`sample` 抓取的 `echo-snapshot-forwarder` 线程中，落在 `peak_gain` 的采样占比显著下降（当前 203/220）。

**复杂度验证**：`Queue::view_entries()` 在 10000 条目下的构造耗时有明确上限（见规格要求），为 O(n²) 实现预留足够的判别裕度。

## Risks / Trade-offs

| 风险 | 缓解 |
|---|---|
| 修订号漏推进 → 队列投影不更新 | 修订号下沉到 `Queue` 的 mutator；删除路径也经由 `Queue` 的 mutator；补一条覆盖全部 mutator 的「每次变更都推进修订号」单元测试 |
| 队列整体替换导致修订号归零 | 统一走 `replace_queue` 辅助函数搬运旧修订号 |
| 队列事件与位置流的先后顺序不确定 | 前端按修订号比较，只应用更大的；`player_sync` 幂等补发 |
| 窗口化渲染改变面板观感 | 复用既有行高与封面尺寸基准；总数照常显示；保留滚动可达性 |
| `snapshot_sequence` 语义变更影响既有前端排序 | 序号仍然单调，前端排序逻辑不变；由 `capture()` 的比较保证「内容相同则序号不变」 |
| 音效记忆化的键覆盖不全导致漏算 | 键取 `(音效文档, 处理环境)`，覆盖 `analyze` 的全部输入；`response_points` 依赖分析结果与采样率，随环境键一并失效 |
| 前后端事件契约短暂不一致（开发期） | 任务顺序保证前端订阅逻辑与后端发射逻辑同批完成；契约变化在 design 中显式记录 |

## Migration Plan

1. 后端先落地修订号、投影拆分与三条事件（`player://snapshot` 的新载荷、`player://queue`、`player://effects`、`player_sync`）。
2. 前端同批改造：订阅三个事件、`playerStore` 拆队列槽位、`QueuePanel` 改读队列槽位并窗口化渲染。
3. 重新生成 IPC 类型，跑 `cargo fmt` / `clippy` / Rust 测试 / 前端 `typecheck` / `lint` / `test`。
4. 真机复测 footprint 与 `sample`，把改动前后的数据写进 PR 描述。
5. 登记场景、同步 delta 到主规格、归档 change。
