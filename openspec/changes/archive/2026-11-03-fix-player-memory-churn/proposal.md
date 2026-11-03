# Proposal

## Why

issue [#112](https://github.com/zsxink/Echo/issues/112) 把主进程 239–361 MB 的周期性内存锯齿归因于「每帧克隆整队两次」，验收项里还写了「拖动重排」这一在代码库里并不存在的功能。按 #112 的描述去改（合并两次 `queue_view()`、裁剪 `UiQueueEntry` 字段）不会让锯齿消失，#112 对风险的外推也偏低。本 change 是对 #112 的实测复核与重新界定，同时补上 #112 完全没提的另一半开销。

**#112 的核心结论是对的，归属是错的。** #112 已经记录了「Malloc Small 稳定在 42 MB 不变」，却没有解释真正在动的那一项。重新在真机上采样（v0.3.1 release，主进程 pid 2109，480 首曲库，前台播放中）后的结论是：

| 观测 | 数值 | 判读 |
|---|---|---|
| `footprint` 采样 | 首次 522 MB，`phys_footprint_peak` 650.8 MB，谷底约 200 MB | 大幅摆动，非单调上升 |
| `heap` 活跃对象 | 125,172 节点 / 22.1 MB | 绝大部分是**已释放但仍计费**的页，不是活对象 → 确认不是泄漏 |
| `Malloc Small` | 59.5 MB 总量 / 43.4 MB 常驻，**全程稳定** | #112 说的两次队列克隆落在这里，**不是**内存主因 |
| `Malloc Large` | 每 2 秒涨约 19 MB，随后掉 50–80 MB，稳态 90 MB → 175 MB（约 20 秒一个周期） | **≈4.25 MB/s 的大块分配churn**，与锯齿 1:1 对应 |
| `vmmap` | 1618 块**恰好 304 KB** 的匿名块，合计 492 MB「已释放但仍计费」 | 尺寸高度一致 → 同一个结构体的重复拷贝 |

**真正的内存主因是载荷体积越过了 macOS 的 128 KB 大块分配阈值。** 10 Hz 的 `player://snapshot` 载荷实测 148,058 字节（480 条队列，均摊 308.5 B/条）加约 10 KB 音效，合计约 163 KB。Tauri 的事件链要复制它三次：

```
serde_json::to_string  →  148 KB
emit_js_script 的 format!  →  148 KB
NSString::from_str（UTF-16）  →  296 KB → 页对齐后 304 KB  ← 实测到的 1618 块
```

三次拷贝都 ≥128 KB，因此全部落进 malloc 的 **large zone**；已释放的页在内核回收前仍计在 footprint 上，于是形成 200–650 MB 的锯齿。量化核对：3 × 148 KB × 10 Hz ≈ **4.4 MB/s**，实测 **4.25 MB/s**，吻合。#112 按「单份 JSON」外推 5 万首时 100 MB/s，实际乘数约 6 倍，**低估**了风险。

**同一条 10 Hz 链路上还有一块 #112 没提的 CPU 开销。** `sample` 抓取的快照转发线程调用图里，220 个采样有 **203 个**落在

```
EffectsState::snapshot → math::analyze → peak_gain → response_db → log10
```

`peak_gain`（`crates/echo-desktop/src/effects/math.rs:153`）用 `INTERVALS = 32768` 做等间隔扫频，每个频点对 10 个 biquad 各算一次 `log10`，即单次约 **327,680 次 `log10`**，再加上每个局部极大值的 `refine_peak`（48 次三分搜索 × 2 次求值）。只要 EQ 非中性（本机为 `builtin:vocal`），这条链每秒要跑 10 次，与队列长度无关。这是快照线程的 CPU 主因，已单独立 issue [#128](https://github.com/zsxink/Echo/issues/128)。

**队列投影本身还是 O(n²)。** `Queue::view_entries()`（`crates/echo-desktop/src/player/queue.rs:377`）用 `Vec::contains` 判重、用线性 `get()` 查条目，随机模式下还要对 `pending` 与 `shuffle_bag` 各做一次线性查找；480 条时每帧约 23 万次 UUID 比较。而且 `queue_provider` 每帧调了**两次** `queue_view()`（`apps/desktop/src-tauri/src/main.rs:559` 与 `:562`）。

**但把上面三条都改掉，播放态的内存增长并没有消失——这是本 change 实施后真机复测才发现的第二个根因。** 拆流之后重新做同条件 A/B（同一份 480 首队列、同一首曲目、同一播放状态、同样 120 秒采样），**改前与改后的两个构建增长曲线完全一致**：

| 构建 | 起始（0–52 s） | 52 s 后 | 终点 `footprint` | `vmmap` 物理占用 | 峰值 |
|---|---|---|---|---|---|
| 改前 | 86–116 MB | `Malloc Large` 4.6 → 375 MB，块数 2 → 96，呈锯齿 | 248 MB | 346.0M | 497.5M |
| 拆流后（只改传输分层） | 58–73 MB | `Malloc Large` 3.6 → 398 MB，块数 1 → 102，呈锯齿 | 216 MB | 325.9M | 471.1M |
| 本 change 完成（拆流 + 封面缓存） | 50–61 MB | `Malloc Large` **不再随帧数增长**，峰值 31 MB / 3 块后回落至 0 | **50 MB** | **49.8M** | **86.3M** |

增长的起点精确落在**换歌**那一刻（位置从 224 s 归零后重新推进），也就是第一帧遇到**有封面的资料库歌曲**。`sample` 抓到的调用链给出了元凶：

```
spawn_macos_now_playing_refresh（10 Hz 快照线程）
  → DiskCoverCache::original → std::fs::read → _xzm_malloc_large_huge
```

`apps/desktop/src-tauri/src/main.rs` 的 macOS Now Playing 刷新线程**每帧**都做三件昂贵的事：把**原始尺寸**封面（实测本机 409 个封面共 158.0 MB，均值 0.39 MB，**最大 4.90 MB**）从磁盘整份读出来；拷进 `NSData`；再解码成 `NSImage` 并新建一个 `MPMediaItemArtwork`，然后 10 Hz 地重新 `setNowPlayingInfo:`。而 Now Playing 的封面展示位只有 512×512——**读原始尺寸的封面去填一个 512 px 的位置**，换来的只有每帧数 MB 的大块分配 churn。#112 把这条链路完全漏掉了：它记录了 `Malloc Small` 稳定，却没有追 `Malloc Large` 的分配来源。

修复后的同条件复测证实了这条链路就是元凶：`sample` 里 Now Playing 线程的 `DiskCoverCache::original` 采样从 **7 次降到 0 次**（拆流后仍是 7 次），播放态 `Malloc Large` 不再随帧数增长，物理占用峰值从 471.1 MB 降到 **86.3 MB**，终点从 216 MB 降到 **50 MB**。

**两个根因是独立的，必须一起修。** 只修传输分层，播放态的内存锯齿照旧；只修封面路径，10 Hz 载荷的大块拷贝照旧。本 change 因此覆盖两者。

## What Changes

1. **把 10 Hz 位置流拆成三条流。** `player://snapshot` 只保留传输状态、当前条目标识与队列修订号，载荷 MUST NOT 再携带队列条目与音效快照。
2. **新增按变更下发的队列事件。** 队列投影只在**队列修订号**变化时整份下发；修订号由队列自身的 mutator 自增，协调器对其持有的失败轮次变化额外自增。
3. **新增按变更下发的音效事件。** 音效快照只在**音效快照序号**变化时整份下发。为此必须先修正 `EffectsService` 现有的 `snapshot_sequence`——它当前是**捕获计数**（每次 `capture()` 都自增），不能直接当变更判据。
4. **音效频响分析不再随位置帧重算。** 派生的频响分析按 `(音效文档, 处理环境)` 记忆化，`peak_gain` 的扫频只在音效真正变化时发生一次。
5. **`Queue::view_entries()` 改为 O(n)。** 成员资格判定与条目查找改用调用内的局部 `HashSet` / `HashMap` 索引，不引入跨方法的新不变量。
6. **`queue_provider` 改为「一次取修订号，仅在修订号变化时构造投影」。** 顺带消除同帧重复克隆，并让当前条目的展示元数据只做一次缓存查询（不再每帧解析整队元数据）。
7. **前端拆槽位。** `playerStore` 增加独立的队列槽位与 `usePlayerQueue()`；`QueuePanel` 改读队列槽位，位置更新不再重建队列数组；队列面板**只渲染可视行及有限余量**，队列总数仍然可见。
8. **新增 `player_sync` 命令。** 前端注册监听后调用一次，由后端补发当前队列投影与音效快照，解决启动期事件早于监听注册而丢失的问题，并作为漂移恢复入口。
9. **macOS Now Playing 的封面数据按封面标识缓存。** 封面**只读取一次、只解码一次**：读取改用尺寸受控的展示档位（512 px）而不是原始尺寸，读取结果按封面标识缓存，已构建的 `MPMediaItemArtwork` 也按封面标识复用。位置帧推进不再重复读盘、重复解码、重复发布封面。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `desktop-playback`：新增「播放快照与队列投影的分层下发」「队列修订号的单调性」「队列投影的构造复杂度上界」「队列投影的初始同步与漂移恢复」四条要求；修改「播放队列面板展示」，补上有界渲染。
- `audio-effects`：新增「音效快照的变更驱动下发」「音效派生分析的输入决定性」两条要求。
- `macos-now-playing-menu`：新增「系统正在播放信息的发布不重复处理封面数据」一条要求。

## Impact

- **行为不变的部分**：队列语义（顺序、身份、当前项判定、优先区、乱序轮次）、音效计算结果（扫频精度、预放余量、频响曲线数值）、会话恢复与持久化语义全部不动。本 change 只改**传输方式**与**计算时机**。Now Playing 的**展示内容**也不变（标题、艺人、专辑、时长、进度、速率、封面都在），只是封面数据的取得方式从「每帧重读重解码」变成「每个封面一次」。
- **IPC 契约变化**（见 design 的「兼容与迁移」）：`player://snapshot` 的载荷去掉 `queue` 与 `effects` 两个字段、新增 `queueRevision`；新增 `player://queue`、`player://effects` 两个事件；新增 `player_sync` 命令。`UiPlayerSnapshot` 的生成类型 `apps/desktop/src/ipc/ipc-types.generated.ts` 必须通过 `pnpm --filter desktop generate:ipc` 重新生成，不得手工修改。
- **场景 ID 位移**：`desktop-playback` 新增 4 条 Requirement，会使后续 Requirement 的场景 ID 顺延。场景登记（`docs/traceability.md`、`scripts/verify/manifest.json`、`scripts/verify/scenario-commands.mjs`）在**实现完成、验证通过、归档时**统一重生成，不得提前登记。
- **规格归属**：`macos-now-playing-menu` 与 `audio-effects` 一样是**尚未登记场景前缀的规划域**，因此新增要求落在各自域的主规格里，不产生场景 ID；其验证由 `cargo test -p echo-app --all-features` 与真机复测承担（见 tasks 第 9 节）。
- **非目标**：不做队列条数上限截断；不做拖动重排（该功能不存在）；不做音效音质改造（见 #109）；不做 5 万首曲库的分页化（见 #114）；不裁剪 `UiQueueEntry` 的字段（本次载荷体积的根因是「整队重复传输」，不是「单条太胖」）；不改 Now Playing 的展示字段与刷新节奏。
