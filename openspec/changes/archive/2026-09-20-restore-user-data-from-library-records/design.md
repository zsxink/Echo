## Context

`portable-library-layout` 已于 2026-09-16 归档，任务 1.1–5.3 全部勾选。但归档的是**用例与适配器**，不是**执行链路**：`InitPortableLibrary`、`ProjectPortableRecords`、`RestoreLibrary`、`write_record_guarded` 四个入口在全仓范围内**零生产调用**（只有各自模块内的单元测试），而 `gen-scenario-manifests` 与追溯表把 `PLL-R03` 的验收记在了只断言歌曲投影的测试上。因此规格宣称的行为与用户实际观察到的行为分叉，且既有门禁无法证伪分叉。

本设计的所有事实均在 **HEAD `9d760be`** 上复验，可在同 HEAD 用文中命令重跑。

### 实测证据

**活动资料库根 `/Users/xian/Music/Echo`**，应用数据 `~/Library/Application Support/com.zsxink.echo/`。
**所有数值以 2026-09-19 23:31:51 的数据库快照为准**（该库在核对期间正被使用，见 E12；快照做法：把 `echo.sqlite`、`-wal`、`-shm` 一起 `cp` 到临时目录再打开，避免只读连接读到缺少 WAL 的陈旧快照）。

| # | 事实 | 实测值 | 复验命令 |
|---|---|---|---|
| E1 | `echo/manifest.json` 不存在 | 缺失 | `ls "/Users/xian/Music/Echo/echo/manifest.json"` |
| E2 | `records/` 下只有 `songs` 与 `favorites` | `playlists`、`playlist-items`、`overrides`、`tombstones`、`play-stats` 五个目录均未创建 | `ls "/Users/xian/Music/Echo/echo/records/"` |
| E3 | 四个 portable 用例无生产调用方 | 全仓 0 处（仅各自模块的 `#[cfg(test)]` 用例） | `rg -n --glob '!target' 'InitPortableLibrary\|ProjectPortableRecords\|RestoreLibrary\|write_record_guarded' .` |
| E4 | song 记录 UUID ∩ SQLite songs UUID | **0**（两边各 87） | 取两侧 UUID 列表后 `comm -12`，或见下方脚本 |
| E5 | favorite 记录 UUID ∩ SQLite `is_favorite=1` | 记录侧 11 条，DB 侧该集合为空（`is_favorite=1` 计数为 0） | `rg -l . echo/records/favorites` 与 `SELECT COUNT(*) FROM songs WHERE is_favorite=1;` |
| E6 | favorite 记录是否为 song 记录的子集 | **是**（差集 0） | 两个 UUID 集合取差 |
| E7 | song 记录的 `media_path` 与磁盘文件 | **87 ↔ 87 双射**：记录有磁盘无 0，磁盘有记录无 0 | 逐个 `is_file()` 校验 + 反向集合差 |
| E8 | favorite 记录的 `is_favorite` 分布 | **3 `true` / 8 `false`** | `rg -o '"is_favorite":(true\|false)' -g '*.json' echo/records/favorites \| sort \| uniq -c` |
| E9 | SQLite 的用户数据计数 | 收藏 0、歌单 0、`playlist_songs` 0、播放 2 次、播放会话 2 条 | `SELECT (SELECT COUNT(*) FROM songs WHERE is_favorite=1), (SELECT COUNT(*) FROM playlists), (SELECT IFNULL(SUM(play_count),0)) FROM songs;` |
| E10 | 重建时间线 | `library_roots.created_at` = 09-19 **23:26:49**；87 首歌 `added_at` **全为 23:26:50**（同一次扫描）；而 87 条 song 记录的 mtime **全为 09-16** | `SELECT created_at FROM library_roots;` + `SELECT added_at FROM songs;` + 记录文件 mtime |
| E11 | `echo/tmp/` 残留 | 89 个条目（导入恢复临时目录从未被回收） | `ls "/Users/xian/Music/Echo/echo/tmp" \| wc -l` |
| E12 | 核对期间该库正在被使用 | 23:22 读到的活动根是 `/Users/xian/Music/测试Echo`（70 首歌 / 59 次播放）；23:26 应用数据目录被重建，活动根变为 `/Users/xian/Music/Echo`（87 首歌 / 2 次播放） | 两次 `SELECT absolute_path FROM library_roots;` 对比 |

E4/E6/E7/E10 合起来给出一个比 issue 描述更精确的结论：

> `echo/` 里的对象资料是**上一代的、自洽且完整**的快照——87 条 song 记录与磁盘上 87 个音频文件严格一一对应，11 条 favorite 记录全部指向这 87 首歌中的歌曲。09-19 23:26:49 应用数据目录被清除（E12 就是用户当时复现 issue 第一步的过程），重开同一目录时扫描在空库上给这 87 个文件铸了**全新 UUID**，于是：歌曲被复制成一份新身份（UUID 交集 0），3 个「我的喜欢」丢失，11 条 favorite 记录变成指向不存在 UUID 的孤儿。**该丢的数据就在 `echo/` 里躺着，只是从没被读回来。**

E2 还说明一件必须对用户讲清楚的事：这个资料库的**歌单无法恢复**——`records/playlists/` 从未存在过，没有任何可回放的副本。本 change 能让「再次打开」不再丢失，但不能把从未落盘过的歌单找回来。因此验收口径必须是「从现在起不再丢失」，而不是「恢复这个库的歌单」。

E11 是本次核对顺手发现的相邻缺陷（`echo/tmp/` 残留未回收），不属于本 change 的范围，记录在此以免日后重复排查。

### 约束

- `docs/DESIGN.md:139` 现在写「SQLite 是本机查询投影和运行时真相源」。该措辞与 issue 的约束（资料库目录是唯一真相源）冲突，必须在本 change 内消除，否则实现者会依据设计文档做出与本 change 相反的选择。
- 一期无远端同步，因此接续只能由**本机打开资料库**这一动作触发，不能依赖「连接同步资料后」（现有 `新设备资料库重建` 的触发措辞）。
- Core 不得依赖 UI/播放器；接续与对账必须落在 `echo-core` 的用例层，desktop 只做装配与调用（见 `docs/DESIGN.md` §2 分层）。
- 本机数据库可被用户随时删除，这是**正常状态**，不是异常：重建路径必须是常规路径而非恢复模式。

## Goals / Non-Goals

**Goals:**

- 让「重新打开一个已存在的资料库目录」成为可恢复路径：对象资料先接续，再扫描 `media/`，UUID 不漂移。
- 让歌单、歌单成员、覆盖层、墓碑、播放统计拥有与歌曲/收藏同等的材料化写入点（目前只有后两者有）。
- 让 manifest 从「可选的、没人写的文件」变成「受管资料库必须存在的头部」。
- 让门禁能证伪「歌单与我的喜欢丢失」——即场景命令必须真的断言歌单/成员顺序/收藏的按 UUID 恢复。
- 消除 `docs/DESIGN.md` 与 issue 之间的真相源措辞冲突。

**Non-Goals:**

- 不实现远端同步、不装配 RemoteConnector、不新增网络依赖（二期范围）。
- 不为既有资料库做破坏性清理：不删除已存在的悬空记录，不做自动墓碑推断。
- 不恢复那些**从未落盘**的数据（例如本机资料库的歌单）。
- 不实现 `snapshot.json` 加速文件（它的缺失不影响正确性，仍可留作后续）。
- 不改 `main.json` 的权限集，也不新增 IPC 命令——除非呈现接续结果确实需要（见 D7）。

## Decisions

### D1. 真相源归属：资料库目录持久、SQLite 可再生

**决定**：资料库目录的 `echo/` 是用户数据的**持久真相源**；本机 SQLite 是**运行时投影**，对已具备控制面的资料库必须可从 `echo/` 重建。`docs/DESIGN.md:139` 与 §5.1 相应改写。

**理由**：这是 issue 的显式约束，也是已有 spec 的既定方向；跨机器拷贝资料库目录、卸载重装、清除缓存三种场景只有这一种模型能同时满足。

**替代方案**：(a) 保持 SQLite 为唯一真相源，额外提供一个「导出/导入备份文件」的手动功能——用户必须记得备份，重装即丢，「唯一真相源是资料库目录」的 spec 要求无法满足；(b) 把 SQLite 直接放进资料库目录——违反 `可携带对象资料` 明令排除 SQLite 文件的要求，且在同步场景下会互相覆盖（`docs/DESIGN.md` §5.1 已论证）。

### D2. 接续时机：在候选准备阶段的扫描之前

**决定**：把「读 manifest → 投影对象资料 → 再扫描 `media/`」插入 `PrepareLibraryCandidate::prepare`，位于现有 `StartScan` 之前。`RestoreLibrary` 的既有四步顺序（records → projection → media scan → hash relink）保持不动，只是被真正接上电并扩展到全部对象种类。

**理由**：`prepare` 已经承担「建立/复用根记录 + 全量枚举 + 候选扫描」，并且失败时旧活动根不受影响——这正是接续需要的失败语义。若把接续放到 `ActivateLibrary` 之后，会出现「旧库已下线、新库视图先空后跳变」的窗口，且要与 root epoch 竞态处理，复杂度反而更高。

**替代方案**：(a) 打到 `ActivateLibrary` 之后的后台任务——视图抖动 + epoch 竞态；(b) 独立的新命令由 UI 显式触发——用户不知道该点，且违背「打开即恢复」。

### D3. manifest 缺失时的自愈语义

**决定**：三种情形分开处理。

1. 无 `manifest.json` 且无 `echo/records/` → 首次启用，按本机派生的资料库逻辑 ID 建立 manifest（`InitPortableLibrary` 的本意）。
2. 无 `manifest.json` 但有 `echo/records/` → **自愈**：以该目录既有对象资料为准建立 manifest，保留全部对象与 UUID，绝不当成新库重扫。
3. 有 `manifest.json` 但 `format_version` 高于本机支持 → **拒绝接续**，资料库降级只读并明确报告，绝不覆盖既有文件。

**理由**：情形 2 正是本机实测的现实（E1+E2+E7）；不区分它就会「修好之后第一次打开仍然把用户数据冲掉」。

**替代方案**：直接在情形 2 覆盖 manifest——会让更高版本的资料库被静默降级破坏（`control_plane.rs` 的 `read_manifest` 已经把高版本视为 absent，若自愈逻辑照 absent 处理就会走到覆盖分支，因此必须在自愈前区分「不存在」与「不兼容」）。

### D4. 播放统计的载荷形态：独立对象种类 + 按设备加性计数

**决定**：新增对象种类 `play-stats`，一条记录对一首歌，载荷形如
`{song_uuid, revision, updated_by_device_id, hlc, by_device: {<device-uuid>: <count>}}`；读取时 `play_count = Σ by_device`。

**理由**：计数必须可加合并。若沿用 `SongRecord` 的 `revision`/`hlc` 语义（单值、LWW），两台设备各自播放同一首歌会互相覆盖，合并后计数**变小**——对「播放次数」这种单调量是不可接受的语义。按设备分桶则合并无损，且天然幂等（同一设备重复写入同一桶只是覆盖为相同或更大的值）。

**替代方案**：(a) 把 `play_count` 直接加进 `SongRecord`——每次播放都要重写携带 `media_path`/`content_hash` 的歌曲记录，且 LWW 丢计数；(b) 每次播放写一条播放会话记录（复用 SQLite 已有的 `recorded_play_sessions` 形状，天然幂等）——文件数随播放次数无界增长，需要额外的压缩/归档策略，一期不值得；(c) 不携带播放统计——issue 明确要求恢复播放次数。

**兼容**：`play-stats` 是新增种类，旧客户端读不到该目录时按「无播放统计」处理即可；`SongRecord` 形状**不变**，因此不需要提升 manifest 格式版本。

### D5. 陈旧与悬空记录的处置：保守、不破坏、以墓碑为准

**决定**：接续时对每个对象按以下规则判定有效性，三条都**不删除**任何文件：

1. 存在墓碑且其版本高于对象记录 → 对象不进入当前有效视图（不复活）。
2. 存在对象记录、无墓碑 → 采纳为有效（这是「最后一次已知状态」，`is_favorite:false` 也是一条有效状态，因此 E8 的 2 条 `false` 不会被误恢复成喜欢）。
3. 记录引用的对象在本机无法成立（例如记录指向的歌曲既无记录也无墓碑）→ 保留文件、不计入有效视图、在本次接续的对账报告里计数。

**理由**：第 2 条是本 change 的核心价值——E4/E5 里那些落在本机数据库之外的记录，在墓碑缺失时**就是用户丢掉的最后状态**，必须采纳（判定它「陈旧」是错的，见 E6/E7：记录与磁盘严格自洽）。第 3 条避免把无法解释的数据当权威，同时避免破坏性清理（本机已有的 11 条 favorite 记录在接续后应全部落在第 2 条，因为它们都是 song 记录的子集，其中 3 条 `true` 会被采纳、8 条 `false` 保持「不喜欢」）。

**替代方案**：(a) 删除所有本机不认识的记录——会销毁用户唯一的副本，绝不可取；(b) 立即在接续中写入墓碑——需要判断「谁删的」，信息不足，会把可恢复状态永久钉死。

### D6. 材料化的落点：跟随既有「提交后写记录」模式

**决定**：所有新写入点沿用 `ImportExecution` 与 `favorites.rs` 已验证的模式——**SQLite 事务提交后**再写对象资料，写失败即让整个用例失败（导入/变更视为未完成），由 `operation_journal` / 恢复流程幂等重驱。

**理由**：既有模式已经把「数据库已提交但记录未写」的不一致窗口交给恢复流程处理；换一套新的两阶段协议只会引入第二种真相。歌单/成员/覆盖层/墓碑/播放统计各自的事务边界需要逐个确认（哪些走 `UnitOfWork`、哪些已经在事务内）。

**替代方案**：在资料库「关闭时统一 flush」——崩溃即丢，且与 `operation_journal` 的逐步提交语义冲突。

### D7. 失败语义与可观察性

**决定**：控制面不可写的资料库维持现状语义（拒绝逻辑变更、只读浏览），但**新增一次可观察的接续结果**：本次打开是否自愈了 manifest、接续了多少对象、有多少对象被判定为无效（D5 第 3 条）。

**呈现方式优先级**：(1) 复用 `choose_library_root` 已返回的 `LibraryRootStatusDto`（若需新增可选字段，走 bridge 生成器重生 `ipc-types.generated.ts`）；(2) 若必须新增命令，则同步 `main.json` 的 permissions 与 `security.rs` 的 `MAIN_WINDOW_PERMISSIONS`，并重跑 `cargo check -p echo-app` 重生 `gen/schemas/`。**倾向 (1)**，把新命令作为 design 尚未定死时的兜底，由实现者按最小改动选择。

## Risks / Trade-offs

- [接续读全量对象资料会拖慢打开资料库] → 对象资料按 UUID 前缀分片、单文件为小 JSON；87 首歌量级可忽略。若未来到十万级需要索引，用 `snapshot.json` 作为可再生加速文件（已在本 change 的 Non-Goals 中显式留给后续）。
- [扩展 `RecordKind` 会影响已归档规格与门禁] → 布局需求已在同一 change 内 MODIFIED；`RecordKind::dir_name` 到目录名的映射是单一实现点，新增种类必须同步规格（否则 `受管理资料库目录布局` 的「每种对象种类都有对应目录」场景会失败）。
- [把悬空记录采纳为有效可能「复活」用户确实删过的东西] → 这是 D5 的已知取舍：墓碑缺失时无法区分「删了但没写墓碑」与「本机数据库丢了」。选择偏向**不丢用户数据**；代价是历史删除可能被带回一次，用户可再次删除（并从此产生墓碑）。
- [门禁改造可能把既有场景命令改红] → `scenario-commands.mjs` 是唯一权威源，改完必须 `gen-scenario-manifests.mjs --write` 重新生成，并跑 `node scripts/verify/run-scenario.mjs <ID>` 与 `pnpm verify:governance`。`check-scenario-churn.mjs` 的 1.8x 棘轮也可能被触发，需要复核新增命令是否让去重后命令数越过上限。
- [「先接续再扫描」会改变既有资料库的首次打开行为] → 这是本 change 的目的；但对**没有控制面的旧资料库**必须保持现行为（直接扫描、铸标识），否则会破坏「不兼容旧资料库布局」场景与一期已验收流程。
- [本机实测结论会随 HEAD 与运行中的库漂移] → 文中每个数值都绑定了快照时刻（2026-09-19 23:31:51）与当时的活动根；E12 是真实发生的漂移（核对期间活动根与计数都变了）。施工前应先复跑 E4/E6/E7，并以**当时的**活动根与快照为准重新取值，不要照抄本文数字。

## 本轮方案自身的复审核正表

本方案由一名独立复核者（新上下文，只给 change 目录 + 仓库路径，不给作者的推理过程）逐条重跑后定稿。表中每项都已在本文件对应位置改正。

| 项 | 本方案原写法 | 独立复核实测 | 若不纠正的后果 |
|---|---|---|---|
| 证据锚点 | 以 `/Users/xian/Music/测试Echo`（70 首歌 / 59 次播放 / 19 条 favorite / 17 true·2 false）为准 | 该目录已不是活动根；活动根在核对期间被换成 `/Users/xian/Music/Echo`（87 首歌 / 2 次播放 / 11 条 favorite / 3 true·8 false） | fixture 与「施工前复验」会照抄一组已经不存在的数字，任务 1.2/1.3 第一步就失败 |
| 时间线 | 数据库重建于 09-19 **19:03** | `library_roots.created_at` = 09-19 **23:26:49**，87 首歌 `added_at` 全为 23:26:50 | 把「重建」定位到错误的时刻，无法与 E12 的运行中复现对上，等于用一个查无实据的时间戳解释因果 |
| 数据库计数 | songs 70 / 播放会话 59 / `SUM(play_count)` 59 | songs 87 / 播放会话 2 / `SUM(play_count)` 2 | 让人以为「DB 里是 70 首、记录是另一批」；真实情况是同一个 87 首的库与它的记录脱节 |
| 因果推断 | 「09-19 重建 → 空库上给这 70 个文件铸新 UUID」 | 结论成立但细节全错：应为「23:26:49 应用数据被清除 → 重开 `/Users/xian/Music/Echo` → 87 个文件铸新 UUID」；且**必须声明该库正在被使用**，否则任何人复跑都会得到不同数字 | 复核者会得出「records 与 DB 属于两个不同的资料库根，因果不成立」的相反结论（本次独立复核确实先得出该结论，经二次取证才排除） |
| 数字漂移的可复现性 | 未声明快照时刻与快照做法 | 只读打开带 WAL 的库会读到陈旧快照；必须 `cp` 三件套后打开 | 结论无法被任何人复跑，是最容易让整份提案失信的一条 |

同时更正独立复核者本人的两处误判，以免后续会话被误导：

| 复核者结论 | 事实 |
|---|---|
| `scripts/verify/checks/check-native-attestation.mjs` 不存在 | **存在**（`scripts/verify/checks/check-native-attestation.mjs`） |
| 「每个任务都有对应的 `checks/task-<id>.mjs`」被写在任务 1.4 | 提案原文并无此表述；实际是 129 个任务 id 中只有 72 个有对应 `checks/task-*.mjs`，其余以直接命令登记。任务 1.4 已按此改写 |

## Migration Plan

1. **兼容基线（必须先做）**：为「无 `echo/` 的旧资料库」保留纯扫描路径；接续只在目录已具备可携带控制面时启用。加一条回归测试固定这一分支。
2. **先只读、不写入**：接续阶段上线时先只做投影与报告（不产生新记录），确认既有资料库打开后 UUID 不再漂移。
3. **再打开材料化写入点**：按对象种类分批接入（歌单与成员 → 覆盖层 → 墓碑 → 播放统计），每批独立提交、独立门禁。
4. **回滚策略**：任一批次出问题时，`echo/` 只增不减（不删除既有文件），回滚实现代码即可回到「记录被写但没人读」的现状——用户数据不会因回滚而进一步损失。这也是 D5「不删除」的理由之一。

## Open Questions

- 对象资料的**写入失败**在歌单重命名/成员拖拽这类高频交互上如何呈现：直接失败（与收藏一致）还是排队重试？后者需要新的持久队列，倾向前者，但需要产品确认可接受度。
- `play-stats` 是否需要按 `recorded_play_sessions` 的存在做交叉校验（播放会话表能否被清空？清空后 `play_count` 是否应回退）？本 change 选择「计数器是权威、会话表是本机日志」，若产品希望两者恒等则需要额外约束。
- E11 的 `echo/tmp/` 残留（89 个条目）说明导入恢复临时目录没有被回收：它属于既有恢复流程的相邻缺陷，本 change 只记录不修（避免与接续改动耦合在同一批次）。若确认它会随接续逻辑一并被触碰，再决定是否纳入。
