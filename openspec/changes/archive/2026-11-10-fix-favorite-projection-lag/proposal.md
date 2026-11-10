# Proposal

## Why

点击 ♡ 收藏，心形闪一下后熄灭，歌曲并未进入「喜欢的音乐」——**必须再点一次才真正收藏上**（issue #152）。

根因已实测确认，是「目录提交」与「SQLite 读模型投影」两段式写盘带来的读模型滞后，被前端的一次权威回读放大成可见的功能缺陷：

1. **目录提交同步、SQLite 投影异步。**
   `crates/echo-core/src/infrastructure/sqlite/local_library_edit.rs:277`
   ```rust
   let async_projection = !cfg!(test) && requested_async_projection;
   ```
   `set_favorite` 传 `requested_async_projection = true`（修复前同文件第 109 行）。生产路径下目录（`echo/` 权威库）**同步提交**，IPC 立即返回 `favorite: true`；而 SQLite 读模型被丢给后台 worker 投影，还要等 `COALESCE_QUIET_PERIOD = 500ms` 的合并静默期（`crates/echo-core/src/application/library_projection_worker.rs:17`）。

2. **写完立刻回读，读到的是旧值。** 在真实 SQLite 生产装配上的复现实测：
   ```
   click 1 returned favorite=true after 42.4ms
     read model immediately after: false      ← 读模型还没跟上
     read model caught up after 500ms: true   ← 正好 500ms，与静默期吻合
   ```
   `all_songs` / `favorites` / `library_counts` 三条读路径此时都仍是旧值。

3. **前端把刚点亮的心形冲掉。** `apps/desktop/src-tauri/src/commands.rs:357` 在 IPC 返回前广播 `library://invalidated` → 前端 `invalidateLibrary()` → `useSongs` 用 `setPage({ songs: result.items })` **整体替换**页面（`apps/desktop/src/features/library/useSongs.ts:141`），滞后的 `favorite=false` 覆盖掉乐观状态。前端复现实测：
   ```
   AFTER INVALIDATION (stale read model): aria-pressed = false
   ```

**为什么现有测试没抓到**：`crates/echo-desktop/src/runtime/services/tests/mod.rs:474` 的 `set_favorite_returns_the_committed_authoritative_song` 跑在 `MemoryDatabase`（`ScanFixture`）上，`cfg!(test)` 为真 ⇒ `async_projection = false` ⇒ 同步投影 ⇒ 永远读到新值。**生产 SQLite 路径的这个滞后窗口目前完全没有测试覆盖**，这本身是交付门槛的缺口。

## What Changes

- **收藏（`set_favorite`）改为同步投影**：把 `set_library_favorite` 的 `requested_async_projection` 从 `true` 改为 `false`，让 SQLite 读模型在目录提交确认后**在同一次 IPC 返回之前**完成投影。收藏返回时，`all_songs` / `favorites` / `library_counts` 已经是新值，前端的权威回读不再拿到旧值，**点一次即生效**。
- **修正 LD-01**：现有 `openspec/specs/library-durability/spec.md` 的 LD-01 写的是「SQLite 投影**可在**后台滞后」，并有「前台不等待 SQLite 追平」的场景。本次变更收窄了这条许可的适用范围，必须先修正规格再实现（见下方「与 LD-01 的冲突」）。
- **补齐回归测试**：新增覆盖**生产 SQLite 路径**的用例，断言收藏写入返回后立即回读三处读路径均为新值；现有 `MemoryDatabase` 用例保持不变（它覆盖的是同步投影语义下的另一条路径）。
- **修复收藏数重复计数**：同一首歌连点两次时 `favorites` 计数变成 2。原因是 `apps/desktop/src/features/library/favoriteMutation.ts`（修复前第 27 行）每次点击都从**滞后的行**重建 lane（`desired: song.favorite`），第二次点击时 `desired` 又是 `false`，于是又 `+1`。修法是让增量基准取该歌曲**最近一次已知意图**，而不是可能滞后的渲染行（见下方「实现过程中修正的两处判断」）。
- **让同步投影的开销不随资料库规模增长**：同步投影把增量投影挪进了收藏的关键路径，而实测发现它的开销**随资料库规模增长**（10,000 首下收藏 P95 55.10 ms，**超出 50 ms 预算**）。根因是 `retained::retry` 每次投影都要解码**全部**保留行并整表扫描。修法是保留目录**按需读取** + 新增迁移 `0030` 的三个索引（见下方「实现过程中修正的两处判断」）。

## 与 LD-01 的冲突（必须先修正规格）

LD-01 现行文本：

> 交互式本地编辑在目录提交确认后 MUST 可立即返回已保存状态，**SQLite 投影可在后台滞后**；SQLite 写锁或投影失败不得阻塞已保存确认。

以及场景「SQLite 投影滞后时连续本地编辑」中的「**前台不等待 SQLite 追平**」。

同步投影与这条 MUST **直接冲突**：它把 SQLite 写锁与投影耗时放进了用户点击的关键路径，与「投影失败不得阻塞已保存确认」的意图也相互抵触（投影失败将直接让收藏命令返回「已保存待投影」，而不是让用户立刻拿到已保存确认）。

因此本 change **必须**在 delta spec 中把 LD-01 的许可收窄为「**目录提交仍可后台滞后，但用户直接感知的收藏等高频小写入必须同步投影**」，并保留原场景对**其它**本地编辑（导入、批量编辑、成员增删）的异步投影许可。

## Non-Goals

- 不改目录（`echo/` 权威库）的提交语义、恢复流程、投影 worker 的合并静默期或重试策略。
- 不把**所有**本地编辑都改成同步投影——导入、批量编辑、歌单成员增删维持异步（它们本就不在用户的单次点击关键路径上）。
- 不改 `CoverCache`、IPC 契约、远端协议、依赖或生成物。
- 不处理 issue #152 之外的其他 issue。

## 实现过程中修正的两处判断

提案阶段的两个判断在实现与实测中被推翻，纠正如下（详见 `design.md` 的 D3、D6）：

1. **「同步投影是规模无关的小开销」——错的。** 最初只在 1,000 首下测得「中位 4.81 ms、与资料库规模无关」。补上 10,000 首的同参数测量后被推翻：同步投影在 10,000 首下中位 **30.02 ms**，收藏 P95 **55.10 ms**，**超出 50 ms 预算**。根因在 `retained::retry`（全目录解码 19 ms + 无索引待处理扫描 21.8 ms + 整表独立根清理 31 ms），与资料库规模**成正比**。因此必须额外做「保留目录按需读取 + 三个索引」，否则本次改动会把收藏推过 `LE-R21-S02` 的 P95 预算。
2. **「同步投影会一并修好计数重复累加」——错的。** 实现 D3 后实测计数**仍然**重复累加（1 → 2）：同步投影消除了**读模型**的滞后，但没有消除**渲染行**的滞后——右键菜单持有的是打开菜单那一刻的快照，与写入竞争的整页重读也会把行换回旧值。因此前端仍需修，且修法不是「lane 跨点击保留 `desired`」（那会把失败写入的意图也留下），而是**记住每首歌最近一次已知意图**（有界映射，上限 512），增量基准取 `intents.get(song.id) ?? song.favorite`，写入失败回滚时同步更新意图。

原 Non-Goals 中「不引入前端状态层来记住用户的收藏意图」一条据此撤销——计数是界面自己维护的乐观状态，其正确性只能在前端保证。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `library-durability`: 修正 LD-01，把「SQLite 投影可在后台滞后」的许可收窄为「仅适用于非单次点击关键的本地编辑」，并新增「收藏等高频小写入 MUST 在返回前完成读模型投影」的要求与场景；同时明确同步投影失败仍按「已保存待投影」报告，不得误报为未保存。
- `library-experience`: 修正 LE-R21 中「不等待 SQLite 投影」的表述（与上一条抵触），并新增「收藏计数不得因重复操作重复累加」的要求与场景。

## Impact

- **代码**：
  - `crates/echo-core/src/infrastructure/sqlite/local_library_edit.rs`（`set_library_favorite` 的投影模式；同步分支与前台追赶分支改走增量投影）
  - `crates/echo-core/src/infrastructure/sqlite/durability_projection/retained.rs`（保留目录按需读取）
  - `crates/echo-core/src/infrastructure/sqlite/migrations/0030_retained_projection_indexes.sql`（新增迁移：三个索引）
  - `crates/echo-core/src/infrastructure/sqlite/connection.rs`（注册迁移 0030）
  - `crates/echo-desktop/tests/favorite_projection.rs`（新增生产 SQLite 路径的回归测试）
  - `apps/desktop/src/features/library/favoriteMutation.ts`（增量基准改取最近一次已知意图）
  - `openspec/specs/library-durability/spec.md`、`openspec/specs/library-experience/spec.md`（规格修正，归档时同步）
- **接口/协议**：`LocalLibraryEditPort::set_library_favorite`、`AppServices::set_favorite`、IPC `set_favorite` 命令、`SongView` 全部不变；远端协议、依赖、生成物无变化。
- **数据库 schema**：**新增迁移 0030**——只在 `library_recovery_records` 上加三个索引，不改动任何表结构或保留行；已发布迁移不改写。
- **运行时**：单次收藏的 IPC 延迟从约 42ms 增加到「目录提交 + SQLite 增量投影」的串行耗时（投影只追平水位之后的增量提交，不重放整库）；歌单/导入等路径不受影响。
- **分层与规范**：仅 Core Infrastructure 的 SQLite 适配器内部模式切换 + 一处前端 store 内部状态修正，依赖方向 Platform → Application → Domain / Infrastructure 不变；不新增跨层依赖或抽象。适用 `openspec/CODE_STANDARDS.md` §5（SQLite 访问集中在存储层、关联数据在单个投影事务中提交、按目录提交顺序幂等追赶、**schema 变化必须包含升级、失败恢复和兼容测试**）、§4.2（错误语义不变）、§8（领域规则修复必须包含能复现问题的测试）。
- **兼容**：旧库、已有收藏事实、恢复流程均不变；`已保存待投影` 的错误语义与重试入口不变。
- **事实基准**：`docs/PRODUCT.md`、`docs/DESIGN.md`、`docs/ROADMAP.md` 的当前阶段范围与验收不受影响；界面交互以 `docs/prototype/` 为准（本次不改界面）。

## 事实校核（当前 HEAD）

| 观察 | 证据 |
|---|---|
| 投影模式由 `cfg!(test)` 决定，集成测试中为同步 | `crates/echo-core/src/infrastructure/sqlite/local_library_edit.rs:277` |
| 收藏走异步投影 | 同文件修复前第 109 行（`set_library_favorite` 传 `true`） |
| 投影合并静默期 500ms | `crates/echo-core/src/application/library_projection_worker.rs:17` |
| 写完立即回读为旧值、500ms 后为新值 | 生产 SQLite 装配复现实测（见 Why 第 2 条输出） |
| 前端权威回读整体替换页面 | `apps/desktop/src/features/library/useSongs.ts:141` |
| Rust 命令在 IPC 返回前广播失效事件 | `apps/desktop/src-tauri/src/commands.rs:357` |
| 现有用例跑在 `MemoryDatabase` 上，测不到滞后窗口 | `crates/echo-desktop/src/runtime/services/tests/mod.rs:474` |
| 连点两次收藏计数变 2 | `apps/desktop/src/features/library/favoriteMutation.ts`（修复前第 27 行，lane 从滞后的行重建） |
| 同步投影的开销随资料库规模增长 | 10,000 首同参数实测：收藏 P95 55.10 ms（1,000 首 29.10 ms）；`ECHO_EDIT_TRACE` 分段计时定位到 `retained::retry` 的三次整表扫描 |
| 保留目录按需读取 + 索引后收藏回到预算内 | 同轮 A/B（Release、200 样本、独占运行）：10,000 首收藏 P95 **49.81 ms**（异步基线 43.24 ms，预算 50 ms）；1,000 首 **42.30 ms** |
| 保留目录按需读取后待处理行数为 0 时不再解码歌曲事实 | 迁移 `0030` 三个索引在真实 10k fixture 上实测：待处理扫描 21.8 → 0.014 ms、独立根清理 31 → 0.05 ms、墓碑查询 6.2 → 0.64 ms |
| 场景 ID 由 spec 中的**位置**派生，中途插入会顶掉后续 ID | 新场景最初插在 LE-R21 第 2 条，`LE-R21-S02`（`package.json` 的 `verify:perf` 指向的性能场景）被顶成 S03；移到末尾后既有 5 条 ID 原样不动，新场景取 `LE-R21-S06` |
