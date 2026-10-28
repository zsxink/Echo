# Design

## Context

动机与边界见 [proposal.md](proposal.md)。2026-10-23 已读取 issue #72（无评论）并核对 `main` 分支实现；issue 的“15 首歌曲 / 2 条 song / 11 条 favorite / 3 条 member / 无 playlist”是现场报告，未访问或改动该用户真实资料库，不能当作本次实测。

证据基线声明：下表行号与“当前行为”均取自 `main`（`2d07e7f`）的已发布实现，不取自本 change 的开发分支。开发分支上的 WIP 不能用于证明现状，也不能作为“已实现”的证据；两者混用会使冲突判断失效。

### 实现证据与规格冲突

| 入口 | 当前可核对位置 | 缺口 |
| --- | --- | --- |
| 导入 | `application/import/execution.rs:321–371` | SQLite 歌曲、资源与 DatabaseCommitted 先提交，再检查控制面、写 SongRecord；journal 不改变权威记录后写的事实 |
| 扫描/标签刷新 | `application/scan/mod.rs:268–317,550–610` | `persist_parsed_index` 更新 SQLite index、lyrics、cover，无 SongRecord 写入；其两个调用入口没有外围材料化 |
| 喜欢 | `echo-desktop/src/runtime/services/favorites.rs:20–47` | 用例先更新 SQLite，随后写 FavoriteRecord；文件写失败能上报，但 SQLite 已变化 |
| 歌单与成员 | `runtime/services/playlists.rs:31–65,119–205` | CRUD/成员先 SQLite 后 portable；多文件写到一半时无共同本地提交边界 |
| 歌单封面 | 同文件 `:76–110`；`domain/library/portable.rs:511–518` | 封面源在应用缓存，SQLite cover key 更新不写 portable；PlaylistRecord 只有名字等字段 |
| 播放统计 | `runtime/player/recorder.rs:58–129` | SQLite record_playback 先提交，再写逐设备统计；上层播放不中断，故不能依靠错误返回保障最新计数落盘 |
| 入站同步 | `src-tauri/src/sync_provisioner/remote_apply.rs`；`sqlite/sync/sync_apply.rs:277–359` | 远端账本、资源声明、业务投影与 inbox 写 SQLite，未经过 portable 持久提交 |
| 对象/媒体 | `apps/desktop/src-tauri/src/sync_provisioner/peer_sync.rs:190–196`、`sync_provisioner/resource_publication.rs`、`resource_candidates.rs` | 对象游标与媒体完成分离已有基础，但媒体 publication journal 不能代替对象持久化 |
| 接续 | `application/continuation.rs:130–162` 及其投影模块 | 依次恢复 song/favorite/playlist/member/stats/override；缺依赖被计 invalid，不会靠后续扫描自动补齐 |
| v2 记录 | `domain/library/portable.rs:159–328`；`infrastructure/filesystem/control_plane.rs` 与 `portable_migration.rs` | 已有 confirmed binding/version/typed alias，但 continuation 仍使用 flat list_records；`control_plane.rs:185–200` 本地更新沿用旧 sync，不能证明新 payload 已确认 |

未写完整前缀的 Core 路径位于 `crates/echo-core/src/`；`runtime/` 位于 `crates/echo-desktop/src/`。实施前重新用 CodeGraph 定位，行号只是本次证据。

`docs/DESIGN.md` §5 已声明 SQLite 可重建。主规格 `portable-library-layout` 的“对象资料接续与对账”却规定同路径不同 UUID 时以 SQLite 行为准；这与 issue 的权威边界冲突，必须修改整个 requirement，不能只增加旁路回填。`docs/PRODUCT.md` / `DESIGN.md` 的旧空目录加入口径也落后于现行 LAN 本地优先合并；`openspec/CODE_STANDARDS.md` §5 同样仍写“首次加入只接受空目录”，本次一并校准，不改既有加入策略、LWW 或撤销业务语义。

## Goals / Non-Goals

**Goal:** on the supported local macOS desktop test setup, favorite and create-playlist click-to-saved-feedback P95 is below 50 ms with real 1,000- and 10,000-song libraries and at least 200 measured operations per action. Record separate P50/P95/P99 for durable directory confirmation and SQLite projection lag. This is an acceptance target, not a guarantee across arbitrary disks or filesystems; report the device, filesystem and run conditions.

**Goals:** 新协议中任何成功保存的资料均有独立于 SQLite 的持久凭据；删除应用数据目录后可从完整新协议音乐库离线重建同一用户资料；崩溃恢复可重复执行；正常重启继续使用 SQLite 加速。

**Non-Goals:** 不修复、救援或回填现有旧音乐库的历史数据、身份或关系；兼容读取支持的旧格式不要求迁移或改写历史记录。不提供音乐文件标签写入或媒体内容替换；标签由 Echo 外部的软件修改，Echo 通过扫描/监听读取变化并保存新的资料库事实。不把派生 FTS、缩略图、媒体探测缓存或本机可用性当作必须逐字保存的数据；它们可重建。系统钥匙串、网络启用意图、主题/EQ/队列不属于音乐库恢复。恢复用户资料不等于无凭据继续原主机账本/旧 epoch；不从音频标签虚构喜欢和歌单。

## Decisions

### D1. 区分业务资料、派生状态和设备信任

| 数据 | 目录内必须持久化 | 重建规则 |
| --- | --- | --- |
| 歌曲 | UUID、库 ID、相对路径、BLAKE3、原 added_at、用户元数据/删除状态 | 标签与探测值可从相同音频重解析；媒体缺失仍保留已保存元数据和关联 |
| 喜欢 | 独立对象的 true **及 false**、版本/删除事实 | 取消喜欢不能因缺记录而被旧 true 覆盖 |
| 歌单/成员 | 名称、创建时间、稳定 UUID、成员 UUID、加入时间、追加顺序、封面资源引用及删除事实 | 空歌单也必须存在；不以成员推测未知歌单名 |
| 覆盖层/资源 | 所有已支持覆盖字段、歌词/自定义封面原始字节与哈希引用 | 缩略图重新生成；应用缓存 key 不是可恢复源 |
| 统计 | 按设备单调桶（共享对象）；幂等播放事件身份（目录私有 state） | 重放不重复加一；设备桶按 max 合并，不合并为 LWW 总数；事件身份不进入共享载荷 |
| 同步必要事实 | confirmed baseline、typed alias、未确认候选、稳定 mutation ID/前驱/冻结时间、冲突双方与所需资源 | 恢复用户状态、候选和幂等语义；无可信会话证据时停止网络并明确重新配对/新 epoch |
| 可撤销删除 | 目录内本机意图、被暂存资源与原关联 | 沿用 10 秒窗口及唯一终态；不序列化为共享最终墓碑 |
| 不保存为共享资料 | 绝对路径、凭据、会话、UI/EQ、SQLite、传输缓存 | 本机路径重选、信任重建；缺失不阻止离线数据恢复 |

扩展已有 portable 字段，不能只把 SQLite row 原样转 JSON。向记录补充缺失的创建/加入时间与内容寻址资源引用，缺历史时间则保留 absent，不伪造。现有音频内嵌封面/歌词可重解析；用户自定义资源必须迁入 `echo/assets/<hash>`，再写对象引用。未来独立 UI change 若新增歌单封面/字段，应复用此持久化路径；其字段和界面不属于本 change。

### D2. 目录提交是唯一用户数据提交点

采用**目录内 redo journal + 单一提交标记 + 可重建投影**。不使用“SQLite commit 后补写”，也不把 SQLite `operation_journal` 作为唯一恢复凭据；它仅是运行期索引。物理多文件写入不能一次原子 rename，逻辑可见性由目录提交协议保证。

```text
<root>/
  media/                         音频与侧车
  echo/
    manifest.json                格式 v3；记录 envelope 仍显式版本化
    records/<kind>/...            已材料化对象；typed tombstone 沿用现有隔离
    assets/<content-hash>         用户资源原件；可恢复资料
    transactions/<operation-id>/  完整 redo 数据、资源声明、提交标记
    state/                       本库候选/确认/删除意图/冲突的可恢复本机资料
    migrations/<migration-id>/    新协议升级与恢复所需的迁移凭据
    snapshot.json                可再生加速文件
    tmp/<operation-id>/           未提交的复制/下载暂存，可按引用规则回收
```

`transactions/` 与 `state/` 不得放进“随时清理”的 tmp，也不得原样网络同步；绑定信息与逻辑设备 UUID 可存在，凭据与端点不得存在。网络仍传现有版本化对象/资源，不传目录私有恢复日志。完整离线备份保留整个 echo；同步/导出对象必须只读取已提交、已恢复的一致版本。

状态机：

```text
Prepared → ResourcesReady → LibraryCommitted → RecordsMaterialized → Projected → Complete
                  ↘ 未提交时可取消/安全回收
LibraryCommitted 之后只能幂等补齐，不能因 SQLite 失败撤回成功事实
```

1. 在资料库级排他写入许可下，验证库身份、格式、可写性和预期对象版本；冻结 operation ID、所有新 UUID、时间、完整 after-images、类型化墓碑、事件 ID/前驱、资源哈希与声明。昂贵复制/解析在许可外有界并发，提交前重验资源与版本。
2. 安全 exclusive-create journal，写全 redo 内容和 checksum，刷文件及父目录；导入资源按既有无覆盖规则发布，校验与刷盘。扫描和 watcher 只读取用户音乐文件，提交前再次确认其大小/哈希；若外部标签编辑器或文件管理器改变内容/位置，则按新文件事实重新准备，复用可证明的 Song UUID。Echo 不替换或写回源音乐文件。
3. 原子发布并刷盘 `committed` 标记，标记绑定整个 journal 内容摘要；这是持久事实提交点。多对象 after-images、候选捕获与逻辑级联属于同一个提交集合，已提交文件残缺视为损坏，不能当作未提交清理。
4. 按 journal 将 records、assets 引用及 state 幂等材料化；写时临时文件 + 刷盘 + 原子替换 + 父目录刷盘。已提交而未材料化期间禁止其他读者/同步导出读取半套 records；重启先恢复 journal。完成后可在 SQLite 同一事务投影业务对象、索引、outbox/inbox、游标及 operation ID/checkpoint。
5. SQLite 完成后返回普通成功并发出视图失效事件。SQLite 不可写则返回 `SavedPendingProjection(operation_id)`，保留提交内容、安排有界重试，界面说明“已保存到音乐库，等待更新视图”；不能报告普通失败诱导用户再创建，也不能报告普通已完成。提交结果不确定（如刷盘返回错误）保留稳定 operation ID 并先核对标记，禁止分配新身份盲重试。
6. journal 仅在 records/state/资源已独立足够重建并刷盘后压缩；SQLite 中已投影标记不是安全回收的充分条件。仍被待确认候选、快照、删除意图/冲突引用的唯一资源不可回收。备份在暂停写入且完成 journal 恢复/材料化的稳定边界复制，恢复时验证完整性。

目录内每次提交还携带独立于主机 `commit_seq` 的本库单调 `library_commit_seq`、前驱提交摘要与库 ID；在排他锁内从目录已验证链头分配，不能从 SQLite 推导。提交标记绑定此序列与前驱，恢复严格从持久材料化检查点之后沿链按序执行，拒绝断链、同序列不同摘要或跨库记录。对象及 state 的材料化水位保存于目录本身；C1 投影失败后即使接受了 C2，也不能让 C1 重试覆盖 C2 的对象值。SQLite 按相同本库序列幂等追赶；不能逐操作随意重放。压缩时先刷盘带链头摘要的完整 records/state 检查点和覆盖清单，再移除已被覆盖的 journal；中断清理可重复，缺检查点不能靠数据库水位授权 GC。此检查点只代表已材料化状态，事务集合仍是其后提交的权威依据。

音乐文件写回和标签编辑不属于本 change：外部软件拥有文件字节的修改流程。Echo 只读取修改后的文件，先提交新哈希/标签 after-image，再投影 SQLite；若目录提交前失败，旧目录事实与投影仍有效，提交后失败则以同一 operation 重试投影。文件移动同样视为外部文件系统变化，只有内容身份可证明时才沿用原 Song UUID。Echo 自有资源（封面、歌词等）的内容寻址发布仍按其资源 journal 执行。

提交标记可靠后，若失败的是 records/state 材料化而不是 SQLite，返回 `SavedPendingRecovery(operation_id, stage)` 并暂停受影响读取/导出，先恢复目录再投影；这同样不是未保存失败。统一以明确失败阶段保留用户已保存事实，不能只有 SQL 错误才区分提交前后。

SQLite 中 `applied_library_operations` / 库投影水位通过**新增顺序迁移**实现；不改已发布迁移。检测 SQLite ahead、库 ID 不匹配或水位无法证明时，以新协议目录事实重建；旧 SQLite 可隔离以供诊断，但本 change 不从中读取或救援历史业务资料。

替代方案：每次整库重写一个 JSON 性能与并发代价过大；只逐文件原子替换没有集合提交；DB outbox 异步补写使重装丢失唯一恢复凭据。选择 redo journal 复用现有文件原子写与 operation journal 经验，明确跨资源边界。

**投影必须按水位追赶，而不是逐操作就地投影。** 每次提交后由协调器从 SQLite 已记录的投影水位出发，按目录链顺序幂等重放尚未投影的提交；单次操作的成功只推进触发点，不能替代追赶。否则 C1 投影失败后 C2 成功，C1 的业务事实将永远不进入 SQLite，“较旧操作延迟重放”无法成立。实现上只有一个投影入口（顺序追赶），per-operation 闭包只是它的触发点，不得各自直接写业务表。

**批量提交与链压缩。** 扫描和批量导入可采用有界批次减少重复持久化成本；首要约束仍是先可靠提交唯一权威资料，不以推迟提交换吞吐。目录链 MUST 有明确的安全压缩策略：当材料化检查点已覆盖某段历史且不再被候选、快照、冲突、删除意图或未完成发布引用时，先刷盘带链头摘要的覆盖清单，再移除已被覆盖的 journal，且不得丢弃未投影提交。面向交互的本地编辑采用下述异步投影路径；恢复和显式追赶仍可同步等待。

#### D2.1 GC 阶段内刷盘去重与状态合并

优化按阶段实施，不能跨 prepared、marker、材料化、coverage 与 GC 合并必要屏障，
不改变保存成功时点，也不跳过完整读取、BLAKE3、解码或当前库归属校验。
首轮保留 GC 对 coverage 和每个 receipt 的文件 `sync_all`：先完成全部文件证明，
收集其唯一祖先目录，在无 create/rename/delete 的 GC 证明阶段按子目录到祖先逐一
sync；全部成功后才允许第一次 journal 删除。删除阶段完成后对 transactions 目录
及其祖先统一 sync；中途失败返回错误，后续恢复重新证明并幂等清理。

目录集合仅属于当前阶段，不跨调用、错误或重试保存。`publish` 的 exclusive 路径
在目录 sync 后还会 unlink temporary，后续 receipt 也会修改共享祖先，因此不能把
成功返回当作所有目录持续有效的证明。本轮不引入文件证明缓存。后续若复用同调用
文件 sync，必须绑定 root、seq/head、目标原生文件身份及精确内容，证明目标未被
替换或写入；无法证明就重新刷盘。不得只凭同名文件或相同摘要免除刷盘。

故障 hook 对应实际发生的 native 边界；目录去重后 hook 数量可减少，但每个必要
文件及唯一目录均须有失败注入点。后面的 receipt 文件或任一目录证明失败，必须
保留全部 covered journal，不能以“较早的 receipt 已成功”为由提前删除。

同步状态合并可以建立 `(StateKind, id) → Vec 位置` 索引，以消除逐项线性查找；
保留初始 first-match、既有 Vec 顺序及新键追加顺序，不改变 canonical digest。
这一步不缓存历史导出，也不改变 export 的取数范围。引用图 GC 与 checkpoint 安全上限仍须满足正确性要求；扫描/导入的大规模性能测量不作为本 change 当前验收门槛，可在后续独立优化中开展。

**本地目录与同步身份。** `domain::library::LibraryId` 是 `manifest.json`、`LibraryBatch`、覆盖检查点和 SQLite 投影水位共同使用的本地目录身份；`domain::sync::LibraryId` 是 `SyncBinding` 内的主机同步库身份，须与 authority、epoch 一起比较。两者独立生成，不能比较 UUID 相等，也不能改写既有 manifest 或历史记录来强制相同。目录提交/重放仍验证本地 manifest、batch、检查点、链和 SQLite watermark 的归属；实时网络入口仍须验证完整当前 `SyncBinding`。envelope 与 state 内的旧 binding、原 mutation 和新 binding 映射是历史证据，按原文保存；其中私有 state 保留为 `RequiresStateRecovery`，不会授权联网。通用 JSON 的 `library` / `library_id` 字段名不能作为跨身份域的递归归属判据。

任务 2.1 已引入 typed state key、含完整 binding/事件身份的不可变历史命名空间及校验；
历史无 typed key 的 payload 仍须保留原文，不能单独授予恢复或联网权限。
`(StateKind, id)` 合并索引不得改变既有 scoped key 的派生与隔离规则。显式 adoption
（任务 7.7）仍未完成；取消错误的 ID 相等检查不等于已经完成历史意图接纳与授权。

#### D2.2 精确发布状态与增量提交基线

LAN 的新 journal 应承载受影响对象的完整 after-image 与新增/改变的 typed state，
不应每次重复携带全部历史。省略的 key 表示本次未改变，不能视为删除；删除必须有
typed tombstone/终态事实。新协议首次材料化和 coverage 可以使用完整快照；不执行旧库历史救援。

先单独优化 `mark_published`：它仅改变一条 ledger 的发布状态。只有目录中已存在
同 root、完整 binding/namespace、sequence、原正文及 digest 匹配的权威 ledger，
且待提交状态除 publication 字段外与之完全相同，才可提交 records 为空、仅包含该
ledger 完整新状态的集合。所有旧冻结正文、资源、receipt、alias、cursor 与其他
binding 保留在既有目录历史中，不在本次提交中重写。无法证明此前 ledger 的历史
路径保留现有完整捕获行为，不能将尚未播种的 SQL 资料默默丢弃。

共用同一个目录提交/不确定操作查询/错误映射入口，避免复制提交协议；原 operation
重试继续读取其已保存的不可变 batch，不重建身份或改写旧 receipt。本步骤不改变
schema 或恢复解释，也不代表 4.2 已完成。

publication 原操作重试还必须核对当前 root/manifest、来源及完整 typed ledger 与
已保存 batch 相同；历史 operation ID 即使碰撞，也不得沿旧 binding 的正文提交
新 authority/epoch 的 SQL 变化。不匹配拒绝并回滚规划事务，原收据保持不变。

通用增量捕获与 host confirmation 的进一步缩减必须先建立 4.2 的共同基线：规划前
按已验证目录 head 追赶 SQLite 水位；冻结计划后目录先提交，再由统一 projector
按水位追赶，原规划 SQL transaction 不得作为另一个业务投影入口。只有与同 root、
manifest、seq/head 对应的基线才能用于省略等值 after-image。不能直接比较未证明的
SQL before/after 来丢弃旧资料；C1 已保存而 SQL 失败时，C2 和重启追赶必须仍保留 C1。

### D3. 用例层统一协调，目录与 SQLite 分别适配

```text
React / Tauri / player recorder / echo-sync adapters
          ↓ typed use cases
Core Application: prepare change / commit library / project / recover
          ↓ ports                              ↓ ports
Filesystem durable commit adapter         SQLite projection adapter
          ↑ immutable domain change sets / records / identities ↑
```

Domain 定义带类型的 change set、来源（本地编辑、扫描、主确认、远端接收、迁移、恢复）、状态/结果，不认识文件句柄、rusqlite 或网络 DTO。Application 通过聚焦 ports 协调资料库提交、资料库读取与 SQLite 投影；沿用 UoW 的数据库原子投影，但禁止业务入口绕过提交用例直接修改用户对象。各 trait 保持 ≤6 个方法，filesystem/SQLite adapters 以及 fault fake 提供真实边界与第二实现需求。

目录持久化能力已经拆分为独立的 `LibraryDurabilityPort`（资产读写、提交、恢复、操作状态查询）。`ControlPlanePort` MUST NOT 再暴露指向同一实现的转发方法；调用方统一经 `control.durability()` 取得该端口。现存的转发方法必须删除，不能以扩大既有 trait 并依赖规模白名单的方式绕过 §2.1。

**只读语义。** 读取记录、清单和资产 MUST NOT 取得排他写锁，也不得因写锁文件不存在而失败：只读卷上的已提交资料必须可浏览。写锁只由写入者在提交、材料化与迁移时取得。

Ports and Adapters 隔离技术；State Machine 表达崩溃恢复；Unit of Work 只保证投影原子；redo journal 解决多文件持久化；Repository 保持读模型。长时间 FS/哈希工作不阻塞 async executor，不跨 await 持有数据库事务。写入许可既串行同库写入也防跨进程旧/新实例并发写；取消、切库、退出都可追踪并等待安全边界。

#### D3.1 本地工作准入与生命周期排空

3.1 的真实取消、切库和正常退出必须经过同一运行实例的本地活动屏障，不能把
`cancel_all()` 的置位、测试线程自行 join 或进程最终释放文件锁当作安全边界。
现有进程级 `LOCAL_ACTIVITY` 读写锁改为显式装配的 `LocalActivity`，由 `ScanDeps`
与 Desktop recorder、services、provisioner 共享同一实例；独立 runtime/test fixture
不得相互关闭准入。不新增 ControlPlane/LibraryDurability trait 方法，不改数据库
schema、目录格式、LAN wire 或 IPC DTO。

屏障使用短暂状态锁和条件变量，状态表达开放、排空和永久关闭。外层租约只在开放
时进入；同一线程对同一屏障的既有嵌套租约在排空期间仍可完成，避免 writer 等待时
嵌套 read 的重入死锁。租约不可跨线程移动，最后一个嵌套租约释放才算该操作结束。
关闭操作不得在自己持共享租约时等待；已经关闭准入时的新独占请求应拒绝，而非
阻塞到使同步 producer 的 stop/join 无法返回。等待期间不持 SQL、播放器或 FS 写锁。

切库固定为：候选准备完成并释放租约 → 现有同步 handoff → 关闭本地新工作准入 →
请求取消 → 排空已准入操作 → 重新读取候选、blockers、活动根与 epoch → 激活事务。
关闭句柄采用 RAII，成功和失败均恢复准入，失败仍由现有 handoff 回退恢复旧 session；
若正常退出已建立同步启动围栏，回退不得重新启动 session。
候选准备的扫描前写入、恢复重试/材料化和回收 worker 的活动根解析也必须在租约内。
扫描注册 token 后重新检查关闭状态，并在最终 missing pass 前重新检查取消；已开始
的目录批次先到达安全边界，再结束为 Cancelled，不中途强杀持久提交。

正常退出必须保持 UI event loop 可响应：首次 ExitRequested 暂缓退出并只启动一个
后台 shutdown；该线程内关闭准入、请求取消，通过显式闭包停止外部 producer，等待
本地工作排空并永久关闭，然后触发原退出码的最终退出。重复请求不得重复 dispatch。
失败不能标为 Ready 或强制退出。系统 picker 不得持活动租约，picker 返回后重新
申请准入，确保退出不因等待需要 UI event loop 的 picker 而死锁。

同步退出另有受 transition 锁保护的永久启动围栏：建立围栏后才停止现有 producer，
启动、恢复、配对及 handoff 回退须在同一 transition 临界区校验围栏，覆盖 Enabled
持久化及 producer 装配。普通停止不设置此围栏，仍可重启；退出清理失败保留围栏，
允许再次清理但禁止恢复生产者。等待 transition 的恢复 worker 不得在持该锁时 join。

验证使用真实 SQLite、RootControlPlane 与业务写入，在目录 marker 前后暂停并调用
生产激活/退出入口；验证旧根/epoch 不提前改变、新工作被拒绝、放行后原操作按同一
身份完成、失败后旧库恢复准入。补齐深层 Unicode 根、真实 permit 跨进程及独立根
隔离测试。这部分只保证已明确装配的活动范围，4.2 的全部写入口统一协调仍单独验收。

删除 `echo-desktop` 内自行读 committed DB 再构造 portable 的重复路径；收藏/歌单/统计/编辑迁往 Core，同步 remote_apply 不再直调 raw DB apply；保留低层投影实现但限制为提交协调器内部调用。清理无消费者 materializer 导出、fake、旧成功态测试与“best-effort”注释，不能留下第二条可写入口。

#### D3.2 本地编辑确认与异步 SQLite 投影

SQLite 仅是读模型，不能在每个操作开始时强制追平到目录头，也不能阻塞“已保存”响应。为此，runtime 为当前活动资料库装配一个单写入者 actor：它持有已验证的目录提交状态、当前本地业务视图、同步绑定/作者、对象版本与 HLC、已冻结 mutation 正文及待投影队列。文件系统目录提交仍是唯一确认边界；SQLite worker 是独立、可重启的顺序投影消费者。

一次喜欢/歌单编辑在 actor 内校验并发版本，按目录权威状态生成完整 after-image 和不可变同步 mutation（包括 binding、author、origin sequence/predecessor、revision、HLC、base version、原始正文及资源引用），再持久提交目录。目录提交确认后，actor 立即更新内存权威视图、放入有界待投影状态并返回 `SavedPendingProjection`/已保存结果；不得等 SQLite writer、材料化对象文件、全链校验、checkpoint 或 GC。投影 worker 按连续目录提交序号将业务行、已应用 operation ID/digest 和水位放入同一个 SQLite 事务。重复通知合并为“检查是否有新水位”，不能丢弃持久队列中的操作。

并发编辑由 actor 按单库序列分配版本。创建歌单、规范化名称唯一性、成员追加顺序和喜欢时间在 actor 视图上校验，保证连续操作在 SQLite 落后时仍能正确看到前一条已保存写入。读己之写从 actor 的权威视图取得；未追平的读模型只能通过带水位的读取接口暴露，调用方不得把旧 SQLite 行覆盖较新的内存值。

后台 worker 使用有界唤醒队列和持久目录链作为工作来源；SQLite 被锁、超时或暂时不可用时按退避重试，向 UI 暴露投影水位/滞后，不回滚已保存状态。磁盘队列容量、SQLite 持续失败和待投影内存上限有明确背压：目录提交前可拒绝新操作并解释原因，已经确认的操作绝不丢弃。重启时先验证目录提交链和必要恢复状态，基于检查点恢复 actor，再从 SQLite 水位重放；SQLite 缺失则重建。队列项不能在单独的 SQLite 提交前被移除。

低延迟确认路径只做本次提交安全证明所必需的文件操作。大范围历史验证、对象 records/ 状态材料化、checkpoint/GC 和全量投影在可恢复的后台/启动阶段处理；但任何影响下一操作语义的状态都必须已包含在目录权威提交和 actor 状态。文件发布后的父目录同步确认前，结果保持“待恢复/结果不确定”；只有目录提交确认后才可返回已保存。actor 崩溃、切库和正常退出必须等已接受提交到达安全边界，退出前保留足以恢复的日志与队列水位。

该优化不得弱化现有完整性策略。运行期间非协作进程直接修改 `echo/` 的检测语义必须保持可证明；文件监听只作提示。若现行每次写入全量验证无法从热路径移出且不改变安全边界，则必须先设计验证缓存/目录锁的等价证明并通过既有篡改测试，不能跳过验证以满足延迟目标。不支持可靠文件发布、同步或排他锁的文件系统必须明确失败。

### D4. 每个入口的先后与成功语义

| 入口 | 本地提交内容 | 后续 |
| --- | --- | --- |
| 导入 | 已发布音频/成功侧车声明、Song、added_at、候选 | 目录提交确认后返回已保存；SQLite 后台投影，侧车失败按既有逐资源结果，不重复分配 UUID |
| 扫描/监听 | 新歌曲或路径/哈希/标签变化后的 Song；真实业务变化才捕获候选 | missing/卷卸载是本机派生可用性，不生成最终删除；无变化扫描不铸新版本或事件 |
| 喜欢/歌单/成员 | 完整对象和独立关系、false 或 typed tombstone；级联一个提交集 | 目录提交确认后立即显示已保存；SQLite 后台顺序投影。批量操作沿既有单项粒度报告，失败不撤销其他已提交项 |
| 播放 | 稳定播放完成事件与该设备计数 | 失败不打断声音，但上报未保存统计并保留可追踪待重试状态；禁止 DB-only 增量 |
| 覆盖/封面 | 原始用户资源及覆盖字段 | SQLite 缩略图等派生索引重建；原件不能只在应用 cache。媒体标签变化由外部软件写入，Echo 只扫描并记录结果 |
| 删除/撤销 | 本机意图与资源 claim；最终化时最终墓碑与关联删除 | 10 秒唯一终态保持，intent 不冒充最终删除；物理回收站独立恢复 |

### D5. 同步只是本地写入的另一个来源

快照分页先验证完整库 ID/binding/epoch、水位摘要、所有页与引用；在目录暂存成完整 baseline commit 后才发布其可见集合。保留已有本地对象与候选，不能先清库/先覆盖 records 再验证；大快照分块写 journal，单一根提交标记决定集合可见，不把整份快照塞进内存。

增量/ACK 保存 confirmed envelope 的 object version、typed alias、墓碑、真实冲突与确认信息；保留比其更新的本地候选，不以确认基线覆盖用户可见候选。目录 state 必须能从 confirmed base + 顺序候选重建当前有效视图和 SQLite outbox；只保存最后一份扁平 SongRecord 不足以重建 pending chain。已发送正文不改写，alias 重映射只作用于未发送候选与读模型。

`records/` 在 v3 库中保存当前有效用户值，`state/confirmed/` 独立保存真正主确认的完整 payload 和 receipt，`state/pending/` 保存候选及前驱。给本地编辑附上旧 sync metadata 不再等价于“该 payload 已确认”；恢复基线只读 confirmed，升级时必须对照 SQLite 确认账本/原始 confirmed payload 验证，不能确认的 v2 envelope 保留原件并标记待核对。

重装后的 pending 分两条恢复路径：可信原 binding、epoch、逻辑作者、凭据与账本均可验证时，按原 mutation/前驱链继续，绝不改已发送正文；出现新作者、新 binding/epoch 或需重新配对时，旧事件只作不可变证据，不直接以新凭据重发。用户走显式重新配对/恢复流程后，先对账主规范对象、typed alias/墓碑和可取得的旧 receipt，排除已接纳（含 ACK 丢失）与已终结意图；无法判定的事件保留待处理，不自动重放。对确认尚需保留的用户意图，持久提交一次 adoption 映射（旧事件→新绑定/作者/事件 ID、基线与转换状态），创建可授权的新事件，保留原对象 UUID、原冻结变更时间、成员顺序及设备统计桶。新事件的基线取对账后的真实版本，冲突仍走既有规则；已发旧正文不变。adoption 本身在目录提交并有稳定身份，任一步中断重复执行不会再创建第二个新事件。普通 SQLite 重建不触发 adoption，更不能把全份当前快照当作新编辑上传。

统计桶保留指本地事实不丢失，不能突破既有 `merge_authorized_play_stats` 的作者授权：新作者 B 不得替旧作者 A 提高尚未确认的 A 桶，也不得把 A 的差额挪进 B 桶重加。仅在原作者身份/授权可验证恢复时继续原桶提交；否则 A 的未确认计数与旧事件完整保留在目录并标记待处理，不自动 adoption 为 B 的统计事件。本次不新增替他人统计桶签名/授权协议；B 后续真实播放仅增长 B 自己的桶，已确认其他桶仍按 max 保留。

接收端元数据提交可早于媒体下载：先存本地对象及所需资源清单，SQLite 只能投影为 missing；全文件校验、安全发布与资源完成标记持久化之后才可用。单调游标不得早于对应本地提交与 SQLite 投影，恢复可根据目录 receipt 重放，绝不能跳过尚未持久的对象。

现有资源发布可能选用 `echo/media/<kind>/<id>/<group>/...` 不可变版本路径（`application/remote_resources.rs:105`），普通 `media/` 扫描无法恢复其本地位置。把“逻辑歌曲相对路径”和“已校验本机资源位置”区分：后者以库内相对路径、资源哈希/长度和 owner/version 存于目录 state，重装后只关联有声明且校验通过的受管版本，不递归扫描 `echo/`，也不放宽任意控制面路径作为歌曲逻辑路径。完整目录备份包含被引用的 `echo/media/`；网络仍按资源声明传字节，不传本机物理布局。 扫描与 typed 导出仅从已完成 publication 的现有歌曲关联取得逻辑路径，不能放宽 `LibraryRelativePath` 或把未声明控制面文件加入枚举；已发布字节的 hash/length 未变时重解析只刷新派生索引，不用嵌入标签覆盖共享值、confirmed envelope 或制造上传候选。真正改变字节仍按现有声明、同一 UUID 与本地编辑规则保存逻辑 after-image，校验和 SQLite 文件位置继续使用实际本机路径。

主机现行“DB 确认事务 → publication”改为“规划确认结果/预留序列 → 本地权威提交 → SQLite 账本投影 → 控制面/所需资源发布与独立安全检查点 → published → ACK/快照/增量可见”。主机的确认序列、幂等 receipt 与连续 published 水位必须与目录提交绑定；DB-only 预留允许作为可丢弃临时状态，不允许成为唯一已确认结果。相同 mutation 不同内容拒绝；相同重试返回相同结果。网络发布仍遵守现有安全检查点，不绕过认证/限流/epoch。

确认到达、资源 journal.finish、数据库 cursor、文件存在任一单项均不足以宣称整库已同步。LAN DTO/wire 规则原则上不变；本地 v3 是单独的格式能力。若实施发现已有 wire 无法表达新增用户资源字段，必须先补充协议版本/协商场景与规格，不能静默发送旧端丢弃的字段。

### D6. 支持旧格式的兼容读取边界

受支持的旧 manifest、envelope 和可携带字段只按其原有语义读取；缺省字段保持 absent，不补造时间、身份、关系或确认信息。读取不得触发 SQLite 历史资料救援、对象回填、身份映射、关系补全或对旧记录的写回。发现无法解释或无法关联的数据时保留原件并标记未解析，普通兼容读取不承诺修复或完整重建旧库。

新协议 manifest 围栏只应用于明确采用新协议的资料库；旧程序不得写入已使用新协议的目录。未来未知格式只读且不覆盖。新协议迁移仅负责其自身协议状态的崩溃恢复，不读取旧 SQLite 来补齐业务对象。将来若实测某个旧库需要修复，另行提出 issue 或专用脚本，不纳入本 change。

### D7. 重建与错误反馈

打开顺序固定：锁/格式验证 → 新协议迁移恢复 → 已提交 journal 重放/清点 → typed envelope/alias/tombstone/confirmed base + pending 依赖解析 → 构建新的 SQLite 临时投影 → 校验新协议提交状态 → 扫描关联媒体 → 原子切换活动投影。旧格式兼容读取不触发历史资料救援或修复。新协议 dangling 记录保留到报告，不直接丢弃成 invalid；活动库切换失败维持旧库与其会话，候选库不能污染旧投影。

**悬空引用不得使重建整体失败。** 投影 MUST 分两段：可解析的对象与关系按顺序入正表；无法解析父引用（缺 Song、缺歌单定义、缺成员）的关系 MUST 保留原件、计入 partial 报告，而不是让单个 `UPDATE`/`INSERT` 撞外键或触发器导致整个事务回退、重建永久卡在待投影。已知悬空项须可从 `library_recovery_records`（按稳定 typed key 读回，不能用 Debug 格式做键）与原件重新尝试；补齐依赖后重试应自动关联，无需用户重新导入。

未知未来格式不改写，损坏对象保留原件并报告 partial；完整恢复需对象及依赖校验均通过。控制面只读时浏览既有可读提交（含只读卷上不取写锁），不产生新 UUID、不写喜欢/歌单/统计/同步对象；不会因为 media 可写就允许 DB-only 修改。SQLite 损坏时先隔离再重建，不因库字段完整就自行删除旧救援文件。

**导入的两段操作身份。** 一次导入包含“预留 claim”与“提交 Song”两个目录操作，二者 MUST 可自识别并互相引用：claim 携带预留 UUID、目标、哈希与 `added_at`，Song 提交携带同一预留 UUID；重试分别查询各自的稳定操作身份，不得因第二次提交失败而重新分配 UUID。界面只展示一个用户可见的导入结果，内部两段不得被报告为两次导入。

IPC 用结构化 result 区分 `NotSaved`、`SavedPendingRecovery`、`SavedPendingProjection`、`Complete`、`RecoveryPartial`；现有 UI 乐观更新在未提交失败时回退，已提交待恢复/待投影不能回退为“未保存”。每个新增错误码 MUST 在前端有明确 message key 映射（不得让 key 字符串直接作为用户文案），并显式进入 `ErrorPolicy` 的重试判定表而非依赖默认值。设置中的资料库区域显示“已保存到音乐库/正在恢复/有未恢复资料/音乐库只读”，诊断明细使用现有错误/详情模式，短 toast 不承担唯一持久故障提示。播放统计失败不中断播放，但必须可追踪并可重试；统计的幂等事件身份存于目录 state，不进入共享对象。

### D8. 验证、规范与清理

`specs/library-durability` 定义 LD-01…LD-10 验收矩阵。测试比较用户状态的规范快照（对象 UUID 集合、相对路径/哈希、added_at、喜欢 true/false、歌单定义、成员 UUID/顺序/时间、资源哈希、覆盖、设备统计桶、typed 墓碑/alias、pending 候选），不只比较行数。统计/候选只重放一次；重建绝不生成等价新上传事件。

故障 fake 逐步拦截复制/资源校验、文件刷盘、目录刷盘、提交标记、材料化第 N 个对象、DB transaction/commit、游标、独立检查点、ACK 与 GC；集成测试用临时目录及独立全新 SQLite。本 change 的短 CI 以受影响 crate、前端相关检查和小规模代表场景为准。三平台原生验证及完整 workspace 验收属于整体发布证据或后续独立验收，不作为本 change 短 CI 的门槛；任何已执行的平台验证都必须记录真实结果，不能由单元测试代替。

适用标准：§2.1 文件/trait 规模；§3.1–3.3 向内依赖/ports/state machine/UoW；§4 错误与阻塞任务生命周期；§5 顺序迁移/无覆盖/UUID/同步；§6 生成 IPC 与错误反馈；§8–§9 契约、架构和可追溯验证。执行命令列于 tasks；本 change 验收包含 `cargo fmt --all --check`、受影响 crate 的 Clippy/测试、前端相关检查、governance、IPC 漂移与小规模代表场景，并运行 `openspec validate --all --strict`。完整 workspace 测试和三平台原生验证保留为整体发布或后续独立证据，不阻塞本 change 的短 CI。不得把未执行的项目描述为通过。不新增网络依赖；如文件锁/平台刷盘需要依赖，实施先记录许可证/MSRV及所需平台证明。

**场景登记与门禁。** 本 change 的 delta 会向主规格新增 requirement 与 scenario，而 `docs/traceability.md`、`scripts/verify/manifest.json` 与 `scripts/verify/scenario-commands.mjs` 是三方可执行验收的事实来源。同步时 MUST 遵守两条编号纪律：新增 requirement 一律追加到目标主规格末尾；新增 scenario 一律追加到所属 requirement 末尾。任何插入都会重排既有稳定 ID（例如 `PLL-R07-S05`、`LL-R02…`），并使既有验收命令指向错误场景。同步后 MUST 重新生成场景清单并跑对账、命令有效性与重复度棘轮，不得只跑 `openspec validate`。

## Risks / Trade-offs

- [新协议与旧客户端不兼容] → 仅对明确使用新协议的目录设置 manifest 写入围栏；旧格式保持兼容读取且不要求历史迁移。
- [目录提交及可靠同步延迟波动] → 前台只做当前提交的可靠发布；大范围材料化、重建和投影异步执行，SQLite 与目录提交分别测量。性能证据必须来自实际桌面前后端链路，独立原型不作为产品达标证明。
- [后台投影积压/actor 状态丢失] → 目录日志是持久工作来源，SQLite 业务行与水位同事务，actor 可从目录恢复；队列有界，持久失败报告水位和背压，绝不丢已确认变更。
- [同步 mutation 依赖 SQLite 规划] → 在同一有序 actor 状态分配作者序列、前驱、revision、HLC 与冻结正文；在切换前用本地/入站同步交错测试对比旧规划器结果，不能从滞后 SQLite 补造。
- [主确认与独立检查点的顺序复杂] → 连续 published fence + 同 mutation receipt + 故障矩阵；缺失可信账本停止原 epoch，而本地用户资料仍可恢复。
- [介质不支持可靠刷盘/原子替换或突然拔盘] → 明确能力与错误；不宣称当前结果已可靠保存；保留 journal 与稳定 operation ID 供重试。
- [多个副本离线编辑后手工覆盖目录] → 普通目录复制不等于版本合并；禁止无证明覆盖已提交资料，继续使用既有同步冲突入口。
- [新协议关系无法解析] → 保留原记录并标记未解析；不臆造父对象，也不扩展为旧库修复。
- [独立 UI change 增加歌单字段] → 后续字段复用本次持久化边界；本次不增加该字段或改动界面。

## 性能优化复盘与实测（2026-10-28）

交互式收藏、建歌单、加入歌单和移出歌单在目录日志提交确认后即可返回已保存结果；SQLite 是可重建投影，后台顺序追平。前台操作需要的前置状态应复用当前已验证的 journal replay：歌单及成员记录从 replay 中解析，精确内容去重优先查询 SQLite 的内容哈希索引；避免每次操作重新枚举/读取整份 portable 记录或重复构建同一歌单快照。新写入若遇到 SQLite 落后，只追赶匹配水位后的验证日志后缀，不能退回对完整历史的每次重放。

导入保留两个必须先于外部副作用的目录操作：预留 claim 与最终歌曲 after-image。实测发现独立 `commit` 会在每个阶段重复执行严格目录恢复和整棵已物化文件树校验，令 MP3/OGG 小文件出现 1.8–2.8 秒长尾。导入已先通过批次快照认证日志，且对本次资源执行独立大小/哈希校验，因此预留和最终歌曲提交改为在独占写会话中复用认证状态；扫描与 watcher 仍走严格独立提交，启动恢复仍负责完整篡改/物化校验。不能为减少延迟而异步化预留 claim、跳过资源校验或把媒体发布置于身份预留之前。导入成功的 SQLite 投影必须同时令歌曲 `Available`，否则目录/歌曲行虽已存在，正常可用歌曲查询仍会隐藏它，直到全库扫描更新可用性。

验证结果与边界：

| 操作 | 样本环境 | 样本数 | P95 |
| --- | --- | ---: | ---: |
| 收藏 | 3,000 首合成记录，Release | 100 | 28.09 ms |
| 建歌单 | 3,000 首合成记录，Release | 100 | 34.04 ms |
| 加入歌单 | 3,000 首合成记录，Release | 100 | 31.81 ms |
| 移出歌单 | 3,000 首合成记录，Release | 100 | 32.49 ms |
| 收藏 | 10,000 首合成记录，Release | 30 | 37.21 ms |
| 建歌单 | 10,000 首合成记录，Release | 30 | 49.35 ms |
| 加入歌单 | 10,000 首合成记录，Release | 30 | 37.87 ms |
| 移出歌单 | 10,000 首合成记录，Release | 30 | 37.89 ms |

四种交互编辑均满足服务命令路径 P95 < 50 ms。10k 测量执行了带 P95 上限断言的基准并通过。六种真实短音频格式（MP3/FLAC/M4A/OGG/Opus/WAV）各导入一次，在 3,000 首基准场景为 424–454 ms；1,000 首带跟踪样本为 122–140 ms。导入数据只是单格式观察，不是 P95，也没有为导入设 50 ms SLO。完整测量口径、前后对比与平台限制见 [交互编辑延迟验证](../../../docs/verification/interactive-edit-latency-2026-10-28.md)。以上 `AppServices`/服务命令基准不等于完整 Tauri IPC、React 到屏幕反馈计时；该端到端验收仍按 task 13.5 单独完成。

## Migration Plan

1. 先实现新协议读取、崩溃恢复和新写入路径验证，再切换生产写入口；所有入口接通之前不宣称完成 library-first。支持的旧格式只做兼容读取，不做旧库清点、救援或回填。
2. 增加 SQLite 新顺序迁移及新协议目录恢复凭据；只对采用新协议的目录启用旧程序写入围栏，不要求将既有 v1/v2 音乐库迁移到新协议。
3. 使用临时空 SQLite 从新协议目录重建并按代表性资料快照校验；部分新协议资料保持可见的未完成状态。历史旧库资料不纳入重建或比较。
4. 正常重启恢复有效本机会话；全新安装无凭据时先提供完全离线曲库，不自动联网、不伪造主权限。主机账本不可验证时按既有流程显式建立新 epoch；未确认本地内容保留供对账。
5. 回滚只允许恢复**整套新协议写入前备份到独立目录**并验证；不能降版本号后用旧程序打开已经接受新协议写入的库。已有新协议用户数据可由支持版本导出，不能静默丢弃。
6. 实现完成并通过全部规定验证后，同步 delta specs 到主规格、复核文档、归档 change，再进入 PR 流程。本次规划不执行这些实施步骤。
