# Design

## D1. 问题的形状：写模型与读模型的两个时钟

收藏一次点击会跨过两个存储层：

```
用户点击 → IPC set_favorite
              ↓
        [目录层] echo/ 权威库 —— 同步提交（唯一持久事实来源）
              ↓
        [读模型] SQLite —— 投影（可重建）
              ↓
        IPC 返回 SongView(favorite: true)
```

现状（`crates/echo-core/src/infrastructure/sqlite/local_library_edit.rs:277`）：

```rust
let async_projection = !cfg!(test) && requested_async_projection;
```

收藏传 `requested_async_projection = true`（`:109`）。于是：

- **目录层**：同步。返回前已可靠提交。
- **读模型层**：异步。提交后只做一件事——`projection_worker.wake()`，把后续投影交给后台线程。

后台线程不是立刻投影的。它有一个 500ms 的合并静默期（`library_projection_worker.rs:17`）：

```rust
const COALESCE_QUIET_PERIOD: Duration = Duration::from_millis(500);
```

设计意图是**正确**的：一次突发里的多条编辑共享一次投影，别为每次点击都付一次全量投影的代价。问题在于这个静默期是**无条件的**，不管这次写入是不是正被用户盯着看。

于是产生了 ~500ms 的「已保存但不可见」窗口。实测（生产 SQLite 装配）：

```
click 1 returned favorite=true after 42.4ms
  read model immediately after: false
  read model caught up after 500ms: true
```

## D2. 为什么窗口会变成「要点两次」而不是「慢半拍」

单有滞后窗口还不够致命——乐观更新本来就允许短暂不一致。致命的是**前端会用滞后的权威值覆盖乐观值**：

1. `apps/desktop/src-tauri/src/commands.rs:357` 在 IPC 返回**之前**广播 `library://invalidated`（携带已提交的 `SongView`）。这是为了跨窗口同步（刘海播放器 ↔ 主窗口）而加的（commit `83ce2eb4` / `4d62b166`）。
2. 前端 `startLibraryEvents()` 收到后调 `invalidateLibrary()`。
3. `useSongs` 订阅该失效信号 → 重查目录 → `setPage({ songs: result.items })`（`useSongs.ts:141`）——**整体替换**页面，而不是合并。

第 3 步是关键：整体替换意味着滞后的 `favorite=false` 无条件覆盖掉刚点亮的乐观心形。用户看到的是「心形闪一下 → 熄灭 → 歌曲不在喜欢列表里」。再点一次时投影已赶上，于是第二次「看起来对了」。

**注意这条链路里前端没有 bug**：乐观更新本身是对的，前端忠实地执行了「重新读取权威数据」。错的是**它读到的那份权威数据是旧的**。

## D3. 决策：收藏走同步投影

把 `set_library_favorite` 的 `requested_async_projection` 从 `true` 改为 `false`。

`plan_library_change` 已有这条分支（`local_library_edit.rs:354` 起）：

```rust
if let Some(projection_worker) = projection_worker {
    let _ = projection_worker.wake();       // 异步：只唤醒，不等
} else {
    project_committed(authority.as_ref(), root, &batch, outcome, |committed| {
        // 同步：在同一次调用里把增量投影进 SQLite
        ...
        durability_projection::project_verified_suffix(connection, root, &replay)
    })?;
}
```

改成同步后，`set_favorite` 返回时 SQLite 已经是新值。

**为什么选同步而不是别的方案：**

| 方案 | 为什么不选 |
|---|---|
| 前端记住乐观值，回读时不覆盖 | 治标。真回滚（写失败）时前端要区分「暂时滞后」与「真失败」，逻辑更复杂；且跨窗口场景仍要靠投影 |
| 跳过自己广播的失效事件（方案 C） | 只解决本窗口；跨窗口仍滞后；且要引入「这次失效是不是我自己发起的」的判断 |
| 让投影 worker 立即唤醒（缩短静默期） | 静默期是**批次合并**的收益来源，改成 0 会让每次点击都触发一次投影风暴；而且窗口只是变小，没有消除 |
| **同步投影（本方案）** | 收藏是小写入，增量投影代价可控；返回后数据即真值，**所有**读路径（列表、喜欢视图、计数、跨窗口）立刻一致，不需要任何补偿机制 |

**代价**：单次收藏的 IPC 延迟从 ~42ms 增加到「目录提交 + 增量投影」的串行耗时。这是把一致性买回来的代价，且只施加在**用户单次点击即可感知**的写入上。

### 实测代价（`ECHO_EDIT_TRACE` 分段计时）

同步投影把增量投影挪进了收藏的关键路径，必须确认它没有把交互延迟推过 `library-experience` 的 P95 预算（LE-R21-S02：1,000 / 10,000 首资料库下收藏 P95 < 50 ms）。

用 `crates/echo-desktop/tests/local_edit_performance.rs` 的 Release 基准（**1,000 / 10,000 首、200 样本、`ECHO_PERF_HISTORY=10`**，与仓库记录基线 `docs/verification/interactive-edit-latency-2026-10-28.md` 同参数）分段计时。

> **第一轮结论已被 10,000 首的实测推翻。** 最初只在 1,000 首下测得「同步投影中位 4.81 ms、最大 8.15 ms」，并据此写下「与资料库规模无关」。补上 10,000 首的同参数测量后，这个结论是**错的**：同步投影在 10,000 首下中位 **30.02 ms**，收藏 P95 **55.10 ms**，**超出 50 ms 预算**。原因见 D3.1。

#### D3.1 同步投影为什么会随资料库规模增长

同步分支走 `durability_projection::project_verified_suffix`，它只投影**增量提交**——增量本身是有界的。但增量投影内部还要做一次「依赖重试」（`retained::retry`），而这一步的成本与**资料库规模**成正比：

`library_recovery_records` 是保留目录，每条被投影过的对象一行，所以它随资料库增长（10,000 首时 10,350 行，其中 10,006 行是歌曲事实）。同步投影的每次重试都做了三件与规模成正比的事：

| 步骤 | 10,000 首实测 | 说明 |
|---|---:|---|
| 全目录解码 | 19 ms | `retained::read` 读并 JSON 解码**全部** 10,350 行，只为找出「还没解决」的行——而此时待处理行数为 **0** |
| 待处理行扫描 | 21.8 ms | `unresolved_reason IS NOT NULL` 无索引，整表扫描 |
| 独立根清理 | 31 ms | 单设备根每次投影都执行的 `DELETE`，带 `row_kind='state'` 过滤，但同样要扫过整张表才能找到那 6 行状态 |

三者合计约 **50 ms**，与实测的 30 ms 中位数一致（各项在不同轮次间有波动）。

**结论**：同步投影的开销**不是**有界的，它随资料库规模增长。「只追平增量提交」只约束了**被投影的提交数**，没有约束**投影时读了多少保留行**。

#### D3.2 修复：让投影按待处理的工作量计费

不改变投影语义，只改变它**读什么**：

1. **保留目录按需读取**（`retained::read_for_retry`）：
   - 待处理的**记录**行（`row_kind='record' AND unresolved_reason IS NOT NULL`）——本轮真正要重试的行；
   - 墓碑行——回答「父对象是否已被删除」；
   - 状态行与资源行——**全量**读入。两者都不随资料库增长（一次扫描一条 claim、一个已发布文件一条资源），而投影确实需要它们：分配索引要在**全部**状态集里配对 mutation 与 disposition，发布完整性也要靠全部资源判定；
   - 歌曲行——只读**待处理发布实际引用到的那几首**。整个歌曲目录随资料库增长，而没有待处理发布的轮次一首都不需要。
2. **三个索引**（迁移 `0030_retained_projection_indexes.sql`）：
   - `library_recovery_records_unresolved`（部分索引，仅覆盖待处理行）——待处理行扫描 21.8 ms → **0.014 ms**；
   - `library_recovery_records_kind(root, row_kind)`——独立根清理 31 ms → **0.05 ms**；
   - `library_recovery_records_record_type(root, json_extract(...))`（部分索引）——墓碑查询 6.2 ms → **0.64 ms**。

效果：待处理行数为 0 的常见情形下，一次投影不再解码任何一条歌曲事实。

**行按 object_key 收集，不是按查询结果直接追加。** 几个窄查询的范围是重叠的：一条待处理的状态行既被「待处理」查询命中，也被「状态/资源」查询命中；一条待处理的墓碑既被「待处理」查询命中，也被「墓碑」查询命中。直接追加会让同一行被保留**两次**——`AllocationIndex` 会把它配对成两个位置，资源完整性判定会把它数两次，而全量读从来不会这样做。因此收集结果放进一个以 `object_key` 为键的 `BTreeMap`，**同一行只保留一次**，且保留顺序仍是目录自身的 object_key 顺序（与全量读一致）。

这条等价性由回归测试锁定：见 task 3.9。

#### D3.3 同轮 A/B 结果

同一台机器、同一份参数（`ECHO_PERF_HISTORY=10`、`ECHO_PERF_SAMPLES=200`、Release）背靠背跑异步与同步两轮。异步轮把 `set_library_favorite` 的 `requested_async_projection` 临时翻回 `true`（即**本次改动之前**的行为），跑完复原。

**收藏 P95（ms）**

| 资料库 | 异步基线（改动前） | 同步 + 保留目录修复（改动后） | 差值 | 预算 |
|---|---:|---:|---:|---:|
| 1,000 首 | **28.13** | **42.30** | +14.17 | < 50 ms |
| 10,000 首 | **43.24** | **49.81** | +6.57 | < 50 ms |

**收藏 P50（ms）**

| 资料库 | 异步基线（改动前） | 同步 + 保留目录修复（改动后） | 差值 |
|---|---:|---:|---:|
| 1,000 首 | **24.72** | **24.87** | +0.15 |
| 10,000 首 | **36.83** | **36.13** | −0.70 |

同步投影把投影挪进了点击路径，**理应更慢**：P95 的差就是这次改动的真实代价——1,000 首 +14.17 ms、10,000 首 +6.57 ms，都仍在 50 ms 预算内。P50 在两个规模上基本持平（+0.15 / −0.70 ms），说明代价集中在**尾部**而非中位：同步投影多做的那一次增量投影是个**有界的尾延迟**，而异步轮的中位之所以更低，是因为它的采样窗口量到的是「提交已返回、投影还没做」的旧行为。

差值在 1,000 首更大、在 10,000 首更小，与直觉相反，但这正是 D3.1 的结论：**按需读取 + 索引让投影代价不再随规模增长**，所以规模越大、同步与异步的相对差距越小（10,000 首下异步轮自己也被未投影的保留目录拖慢了尾部）。

#### D3.4 结论

1. **同步投影的开销随资料库规模增长**，除非把保留目录的读取改成按需。1,000 首下只有 4.8 ms，10,000 首下是 30 ms——只看 1,000 首会得出错误结论。
2. **P95 抖动来自 `directory_commit`**，它没有被本次改动触碰：同轮之间 P95 在 27–42 ms 之间摆动，而未被改动的 `create_playlist` 也随之变化。
3. 因此**不能只看一次运行的 P95**，也不能只看一个规模。判定必须用同轮 A/B **且覆盖两个规模**，且要能把「本次引入的开销」与「机器抖动」分开。
4. **保留目录按需读取 + 三个索引把同步投影的代价压回预算内**：10,000 首下收藏 P95 从修复前的 55.10 ms 降到 **49.81 ms**（预算 50 ms）。相对异步基线，1,000 首贵 14.17 ms、10,000 首贵 6.57 ms——**同步投影把「等投影」的时间换成了「做投影」的时间，但没有把预算撑破**。
5. **基准脚本整体退出码不能作为本 change 的判据**。四轮的退出情况：

   | 轮次 | 脚本结果 | 失败项 | 收藏 P95 |
   |---|---|---|---:|
   | 同步 1,000 首 | FAILED | `add_to_playlist` 56.47 ms | **42.30** ✓ |
   | 同步 10,000 首 | FAILED | `add_to_playlist` 51.45 ms | **49.81** ✓ |
   | 异步 1,000 首 | **ok** | — | 28.13 ✓ |
   | 异步 10,000 首 | FAILED | `create_playlist` 53.95 ms、`add_to_playlist` 71.52 ms | 43.24 ✓ |

   **收藏在四轮里全部低于 50 ms**。失败项 `add_to_playlist` 与 `create_playlist` **都仍走异步投影**，本次改动没有碰过它们——异步 10,000 首那轮 `add_to_playlist` 到了 71.52 ms，比同步轮还差，说明它们的尾部抖动与本次改动无关。
6. `remove_from_playlist` 的最大值（四轮 27,736 / 29,572 / 33,444 / 44,536 ms）是基准在每次成员采样前调用 `flush_library_projection(root, 30s)` 撞上 **30 秒投影等待超时**的停顿，不是编辑延迟。它把四轮的最大值全部拉爆，因此最大值这一列在本 change 中没有参考价值。
7. 因此本 change 的判定依据是**收藏这一条路径的 P95**：同步轮 1,000 首 42.30 ms、10,000 首 49.81 ms，两档都在预算内，且 10,000 首那档相对异步基线只贵 6.57 ms。

（仓库记录基线见 `docs/verification/interactive-edit-latency-2026-10-28.md`：1,000 首时收藏 P95 25.07 ms、10,000 首 37.94 ms。）

### 为什么不是所有本地编辑都同步

导入、批量编辑、歌单成员增删都在用户点击的关键路径**之外**（它们有自己的进度反馈与批量结果提示），而它们的投影代价随数据量增长。把它们也改成同步会显著拖慢大库操作。

所以同步投影是**按写入类型**选择的，不是全局开关。`PlaylistEdit::Rename` 已经是同步的（`local_library_edit.rs:178` 的 `matches!(&edit, PlaylistEdit::Create)` 只给 Create 异步），说明这个模式在代码里已有先例。

## D4. 同步投影的错误语义

同步投影失败时，`project_committed`（`crates/echo-core/src/application/library_commit.rs:89-99`）把投影错误包成 `Error::saved_pending_projection`：

```rust
match project(&committed) {
    Ok(value) => { outcome.status = CommitStatus::Complete; ... }
    Err(error @ Error::LibrarySavedPending { .. }) => Err(error),
    Err(error) => Err(Error::saved_pending_projection(operation_id, error)),
}
```

即：**目录已提交 ⇒ 收藏是真的**，只是 SQLite 视图没跟上。这正是 LD-01「目录已提交而投影失败」场景要求的「已保存待投影 + 稳定操作身份」，前端已有完整的处理路径（`savedPendingNotice.ts` → toast + 恢复按钮 → `retry_library_operation`）。

同步投影没有把这个语义改坏，只是把它从「后台发生」变成「前台可见」。delta spec 的「收藏的投影失败仍报告为已保存待投影」场景即锁定这一点。

**注意**：这意味着同步投影失败时，用户会看到「已保存待更新」而不是一个普通的成功。这比现在的「假成功（心形灭了）」更诚实。

## D5. 同步投影只追平增量

`plan_library_change` 的同步分支调用 `durability_projection::project_verified_suffix(connection, root, &replay)`。`replay` 来自 `session.replay()`（目录写会话持有的已验证链），而 `project_verified_suffix`（`durability_projection.rs:172`）只投影**自当前投影水位起**的提交：

```rust
for commit in &replay.commits {
    if commit.library_commit_seq <= seq { continue; }
    ...
}
```

`seq` 来自当前投影水位。因此一次收藏只投影自水位起的**增量提交**，不会重放整库历史。这满足 CODE_STANDARDS §5「按目录提交顺序幂等追赶」。

`project_verified_suffix` 在水位落后于 checkpoint 时会**内部回退**到全量路径（`project_commits`），所以冷启动或刚压缩过的资料库仍能拿到完整投影——回退条件是「水位落后」，而不是「有增量」，因此正常编辑不会触发全量重放。

> 实现说明：同步分支与**前台追赶分支**都改走 `project_verified_suffix`。最初同步分支走 `project_commits`（每次都重验整条链，把一次点击变成 O(资料库历史) 的工作量）；两者统一后语义一致，也避免「追赶用增量、同步用全量」这种不必要的分工。

delta spec 的「收藏写入只追平增量投影」场景锁定这一点。

## D6. 收藏计数的重复累加

这是本次分析中发现的**独立缺陷**，与投影滞后同源但不是同一个问题。

`apps/desktop/src/features/library/favoriteMutation.ts`（修复前的 25–33 行）：

```ts
let lane = lanes.get(song.id);
if (!lane) {
  lane = { confirmed: song, desired: song.favorite, latest: 0, tail: null };
  lanes.set(song.id, lane);
}
...
bumpLibraryCount("favorites", Number(favorite) - Number(state.desired));
state.desired = favorite;
```

lane 的 `desired` 是**增量基准**。但 lane 的创建时机是「这首歌还没有 lane」，此时用的是**渲染行里的 `song.favorite`**——而这个值在滞后窗口内是旧的。

连点两次「喜欢」：

| 步骤 | 渲染行 `song.favorite` | lane 创建 | `desired` 基准 | 计数增量 |
|---|---|---|---|---|
| 第 1 次点击 | false（真实） | 新建 | false | +1 → 计数 1 |
| 第 2 次点击 | false（滞后，仍是 false） | 新建（lane 已被删） | false | +1 → 计数 2 |

第 1 次点击后 lane 在 `finally` 里被删除（`:60`：`if (state.latest === mutation && lanes.get(song.id) === state) lanes.delete(song.id)`），所以第 2 次点击会**重建** lane，重新读到那个滞后的 `false`。

**修复方向**：计数增量不应依赖「渲染行的当前值」，而应依赖**该歌曲最近一次已知意图**。同步投影落地后，滞后窗口消失，重建的 lane 读到的就是真值——**这个缺陷会被 D3 一并修好**。

因此 delta spec 仍单独立一条「收藏计数不得因重复操作重复累加」的要求 + 场景：它锁定的是**用户可观察的结果**（连点两次计数为 1），而不是某一种实现方式。无论后续实现如何调整，只要这个场景成立即满足。

> 实现时若发现同步投影后计数已自然正确，本条场景即为回归防护而非新增逻辑；若仍有问题，再单独修 lane 的基准来源。

### 实测：同步投影并没有修好它

按上述「预期」实现 D3 后，用一个前端用例直接观测计数（`favoriteMutation.test.ts`）：

```
第 1 次点击（写入返回 favorite=true）→ 计数 1
第 2 次点击（渲染行仍是 favorite=false）→ 计数 2   ← 仍然重复累加
```

**根因**：前端的「滞后窗口」并不只有 SQLite 投影滞后一个来源。渲染行同样可能是旧值：

- 右键菜单持有的是**打开菜单那一刻的快照**（`LibraryWorkspace.tsx:74` 的 `menuFor.song`），`useSongs.patchSong` 只改 `page.songs`，**不改** `menuFor.song`。
- 任何与写入竞争的整页重读（`useSongs.ts:141` 的 `setPage({ songs: result.items })`）会把行换回旧值。

也就是说，**同步投影消除了读模型的滞后，但没有消除渲染行的滞后**。前端的增量基准仍可能取自旧行。

**修复**：把「该歌曲最近一次已知意图」显式记下来，作为增量基准，独立于渲染行：

- `favoriteMutation.ts` 新增一个有界的 `intents` 映射，记录每首歌最近一次写入的意图。
- lane 建立时的基准取 `intents.get(song.id) ?? song.favorite`——**已知的意图优先于可能滞后的行**。
- 写入失败回滚时同步更新 `intents`，否则下一次点击会基于一个从未成立的意图计数。
- 映射按最近使用顺序淘汰，上限 512 首，避免无界增长。

这是 D7「不改前端」的一处例外，理由是：**计数是前端自己的乐观状态**，它的正确性只能在前端保证。Core 修的是读模型的正确性，无法保证前端的增量基准取自真值。修复不引入补偿层，也不改变前端与 Core 的契约——只是让前端记住自己刚写过什么。

## D7. 为什么不改前端

前端在这条链路上没有做错任何事：

- 乐观更新是对的（immediate feedback）。
- 收到失效事件后重读权威数据是对的。
- 广播失效事件是为了跨窗口（刘海播放器），也是对的。

前端唯一「可疑」的地方是 `useSongs` 整体替换页面，但那在正常情况下是**正确**的：权威读模型应该整体决定页面内容。若读模型是准的，整体替换就没问题。

所以修复点在**让权威读模型变准**，而不是在前端加补偿层。这符合 CODE_STANDARDS §6「资料库、同步、导入和冲突规则留在 Core」。

**唯一的例外**是 D6 里的计数基准：那是前端自己维护的乐观状态，Core 无从保证它的基准取自真值，因此在前端修（见 D6 末节）。

## D8. 测试策略

### 现有测试为什么测不到

`crates/echo-desktop/src/runtime/services/tests/mod.rs:474`：

```rust
let fixture = ScanFixture::new();      // MemoryDatabase
let app = services(&fixture);
let view = app.set_favorite(ids[0], true).expect(...);
assert!(view.favorite, "returned view reflects the committed write");
let favs = app.favorites(SongSort::default(), None, 100).expect("favs");
assert_eq!(favs.items.len(), 1);       // 读回来是新的
```

`MemoryDatabase` + `cfg!(test)` ⇒ `async_projection = false` ⇒ 同步投影 ⇒ 读回来永远是新的。**这个用例测的是同步投影语义下的行为，而生产是异步**——它无法暴露生产缺陷。

更根本的是：即使把它改成 `MemoryDatabase`，`cfg!(test)` 仍然为真，异步分支永远不会被这段测试触达。

### 新增测试必须跑在生产 SQLite 装配上

新增一个 `crates/echo-desktop/tests/` 下的集成测试，用 `assemble()`（生产组合根，`crates/echo-desktop/src/runtime/app.rs:61`）构造真实 SQLite 运行时，断言：

1. `set_favorite` 返回后**立即** `all_songs` / `favorites` / `library_counts` 全部为新值。
2. 同一首歌连点两次，收藏计数仍为 1。

注意：集成测试里 `cfg!(test)` **对 echo-core 为假**（`crates/echo-core` 是依赖 crate，编译时不带 `cfg(test)`），所以 `async_projection` 取 `requested_async_projection` 的真值——这正是生产行为。这一点在记忆里已有条目（`cfg-test-false-for-integration-tests.md`）记录。

### 不引入时间敏感的断言

不写「500ms 后应该变 true」这类墙钟断言（跨平台 runner 必 flake）。只断言**返回时刻**的读路径已是新值——这是同步投影的确定性契约，不依赖时序。

## D9. 兼容性

- 旧库、已有收藏事实、恢复流程、投影水位全部不变。
- `已保存待投影` 的错误码、IPC 契约与前端恢复入口不变。
- 目录层提交语义不变（始终是同步且可靠的）。
- 投影 worker 本身不变——它仍服务导入、批量编辑、成员增删与启动追赶。
- **既有场景 ID 一个都不变。** 场景 ID 由 spec 中的**位置**派生（`<AREA>-R<NN>-S<NN>`），中途插入一条场景会把其后所有场景的 ID 顶掉一位。本次新增的 LE-R21 场景最初被插在该 Requirement 的第 2 条，结果 `LE-R21-S02`——`package.json` 的 `verify:perf` 与 `docs/native-attestation-playbook.md` 都指向它，作为性能预算场景——被顶成了 S03。修正办法是把新场景追加到该 Requirement 的**末尾**：五条既有场景 ID 原样不动，新场景取 `LE-R21-S06`；`LD-R01` 的三条新场景本来就追加在末尾，`LD-R01-S01..S03` 未受影响。新增 Requirement（收藏计数）追加在文件末尾，取 `LE-R22`。

## D10. 适用层级与规范条款

- **层级**：Core Infrastructure（SQLite 适配器）内部模式切换；Core 不依赖 UI / 播放器，依赖方向 Platform → Application → Domain / Infrastructure 不变。
- **规范条款**：`CODE_STANDARDS` §5（SQLite 访问集中在存储层；关联数据在单个投影事务中提交、按目录提交顺序幂等追赶）、§4.2（错误语义不变）、§4.3（后台任务生命周期不变）、§8（领域规则修复必须包含能复现问题的测试；文件测试用临时目录与固定样本）、§9（规格复核与残留清理）。
- **不新增**：跨层依赖、trait 方法、IPC 命令、依赖、生成物。
- **schema**：**新增迁移 `0030_retained_projection_indexes.sql`**——只在 `library_recovery_records` 上加三个索引，**不改任何表结构、不动任何保留行**；已发布迁移不改写。按 §5「schema 变化必须包含升级、失败恢复和兼容测试」，`durability_migration.rs` 的 `migration_0030_adds_the_retained_projection_indexes_without_touching_retained_rows` 覆盖三件事：部分迁移失败后回滚（无索引残留、保留行不变）、真实升级建出三个索引且保留行不变、重开后 `quick_check()` 通过。
