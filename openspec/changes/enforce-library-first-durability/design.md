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

**Goals:** 任何成功保存的资料均有独立于 SQLite 的持久凭据；删除整个应用数据目录后离线恢复同一用户资料；崩溃可重复恢复；旧数据库尚存时先救援其唯一资料；正常重启继续使用 SQLite 加速。

**Non-Goals:** 不把派生 FTS、缩略图、媒体探测缓存或本机可用性当作必须逐字保存的数据；它们可重建。系统钥匙串、网络启用意图、主题/EQ/队列不属于音乐库恢复。恢复用户资料不等于无凭据继续原主机账本/旧 epoch；不从音频标签虚构喜欢和歌单。

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

扩展已有 portable 字段，不能只把 SQLite row 原样转 JSON。向记录补充缺失的创建/加入时间与内容寻址资源引用，缺历史时间则保留 absent，不伪造。现有音频内嵌封面/歌词可重解析；用户自定义资源必须迁入 `echo/assets/<hash>`，再写对象引用。后续 UI change 新增的歌单封面/字段必须复用此路径。

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
    migrations/<migration-id>/    原始记录备份、救援快照、映射、审计报告
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
2. 安全 exclusive-create journal，写全 redo 内容和 checksum，刷文件及父目录；导入/写回资源先按既有无覆盖规则发布，校验与刷盘。扫描不改动用户音频，但再次确认其大小/哈希，变化则重新准备。watcher 使用同一 claim/预留 UUID。
3. 原子发布并刷盘 `committed` 标记，标记绑定整个 journal 内容摘要；这是持久事实提交点。多对象 after-images、候选捕获与逻辑级联属于同一个提交集合，已提交文件残缺视为损坏，不能当作未提交清理。
4. 按 journal 将 records、assets 引用及 state 幂等材料化；写时临时文件 + 刷盘 + 原子替换 + 父目录刷盘。已提交而未材料化期间禁止其他读者/同步导出读取半套 records；重启先恢复 journal。完成后可在 SQLite 同一事务投影业务对象、索引、outbox/inbox、游标及 operation ID/checkpoint。
5. SQLite 完成后返回普通成功并发出视图失效事件。SQLite 不可写则返回 `SavedPendingProjection(operation_id)`，保留提交内容、安排有界重试，界面说明“已保存到音乐库，等待更新视图”；不能报告普通失败诱导用户再创建，也不能报告普通已完成。提交结果不确定（如刷盘返回错误）保留稳定 operation ID 并先核对标记，禁止分配新身份盲重试。
6. journal 仅在 records/state/资源已独立足够重建并刷盘后压缩；SQLite 中已投影标记不是安全回收的充分条件。仍被待确认候选、快照、删除意图/冲突引用的唯一资源不可回收。备份在暂停写入且完成 journal 恢复/材料化的稳定边界复制，恢复时验证完整性。

目录内每次提交还携带独立于主机 `commit_seq` 的本库单调 `library_commit_seq`、前驱提交摘要与库 ID；在排他锁内从目录已验证链头分配，不能从 SQLite 推导。提交标记绑定此序列与前驱，恢复严格从持久材料化检查点之后沿链按序执行，拒绝断链、同序列不同摘要或跨库记录。对象及 state 的材料化水位保存于目录本身；C1 投影失败后即使接受了 C2，也不能让 C1 重试覆盖 C2 的对象值。SQLite 按相同本库序列幂等追赶；不能逐操作随意重放。压缩时先刷盘带链头摘要的完整 records/state 检查点和覆盖清单，再移除已被覆盖的 journal；中断清理可重复，缺检查点不能靠数据库水位授权 GC。此检查点只代表已材料化状态，事务集合仍是其后提交的权威依据。

显式写回原文件时，发布新字节前必须持久保留旧版本及完整目标意图；未提交状态恢复旧关联或继续相同操作，已提交状态完成新版本，不能因原子替换成功就丢掉旧版本和所有中断证据。

提交标记可靠后，若失败的是 records/state 材料化而不是 SQLite，返回 `SavedPendingRecovery(operation_id, stage)` 并暂停受影响读取/导出，先恢复目录再投影；这同样不是未保存失败。统一以明确失败阶段保留用户已保存事实，不能只有 SQL 错误才区分提交前后。

SQLite 中 `applied_library_operations` / 库投影水位通过**新增顺序迁移**实现；不改已发布迁移。检测 SQLite ahead、库 ID 不匹配或水位无法证明时，以目录事实重建，旧 DB 隔离备份，仅迁移阶段可读取其历史救援快照。

替代方案：每次整库重写一个 JSON 性能与并发代价过大；只逐文件原子替换没有集合提交；DB outbox 异步补写使重装丢失唯一恢复凭据。选择 redo journal 复用现有文件原子写与 operation journal 经验，明确跨资源边界。

**投影必须按水位追赶，而不是逐操作就地投影。** 每次提交后由协调器从 SQLite 已记录的投影水位出发，按目录链顺序幂等重放尚未投影的提交；单次操作的成功只推进触发点，不能替代追赶。否则 C1 投影失败后 C2 成功，C1 的业务事实将永远不进入 SQLite，“较旧操作延迟重放”无法成立。实现上只有一个投影入口（顺序追赶），per-operation 闭包只是它的触发点，不得各自直接写业务表。

**批量提交与链压缩。** 首次扫描与批量导入 MUST 把同一逻辑集合合并为有界数量的提交（例如按扫描批次分组），而不是每首歌一次提交；单次编辑不得触发三次以上全链扫描。目录链 MUST 有明确的压缩策略：当材料化检查点已覆盖某段历史且不再被候选、快照、冲突、删除意图或未完成发布引用时，先刷盘带链头摘要的覆盖清单，再移除已被覆盖的 journal，使链长度与活跃窗口而非总历史成正比。超过安全上限时必须拒绝写入并报告，不得静默降级或丢弃未投影提交。

**本地目录与同步身份。** `domain::library::LibraryId` 是 `manifest.json`、`LibraryBatch`、覆盖检查点和 SQLite 投影水位共同使用的本地目录身份；`domain::sync::LibraryId` 是 `SyncBinding` 内的主机同步库身份，须与 authority、epoch 一起比较。两者独立生成，不能比较 UUID 相等，也不能改写既有 manifest 或历史记录来强制相同。目录提交/重放仍验证本地 manifest、batch、检查点、链和 SQLite watermark 的归属；实时网络入口仍须验证完整当前 `SyncBinding`。envelope 与 state 内的旧 binding、原 mutation 和新 binding 映射是历史证据，按原文保存；其中私有 state 保留为 `RequiresStateRecovery`，不会授权联网。通用 JSON 的 `library` / `library_id` 字段名不能作为跨身份域的递归归属判据。

当前 state 仍是按 `(StateKind, id)` 覆盖的未类型化 payload；完整 typed state 契约（任务 2.1）和显式 adoption（任务 7.7）尚未完成。后续必须定义含完整 binding/事件身份的不可变历史命名空间和有类型校验，避免不同 binding 的同 kind/id 历史相互覆盖；取消错误的 ID 相等检查不等于已经解决这些历史保留与授权问题。

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

删除 `echo-desktop` 内自行读 committed DB 再构造 portable 的重复路径；收藏/歌单/统计/编辑迁往 Core，同步 remote_apply 不再直调 raw DB apply；保留低层投影实现但限制为提交协调器内部调用。清理无消费者 materializer 导出、fake、旧成功态测试与“best-effort”注释，不能留下第二条可写入口。

### D4. 每个入口的先后与成功语义

| 入口 | 本地提交内容 | 后续 |
| --- | --- | --- |
| 导入 | 已发布音频/成功侧车声明、Song、added_at、候选 | SQLite 投影后普通成功；侧车失败按既有逐资源结果，不重复分配 UUID |
| 扫描/监听 | 新歌曲或路径/哈希/标签变化后的 Song；真实业务变化才捕获候选 | missing/卷卸载是本机派生可用性，不生成最终删除；无变化扫描不铸新版本或事件 |
| 喜欢/歌单/成员 | 完整对象和独立关系、false 或 typed tombstone；级联一个提交集 | 批量操作沿既有单项粒度报告，失败不撤销其他已提交项 |
| 播放 | 稳定播放完成事件与该设备计数 | 失败不打断声音，但上报未保存统计并保留可追踪待重试状态；禁止 DB-only 增量 |
| 覆盖/封面/写回 | 原始用户资源、覆盖字段；写回后的媒体哈希与对象 | SQLite 缩略图等派生索引重建；原件不能只在应用 cache |
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

### D6. 旧库回填是受控的一次救援，不是永久 DB 优先

顺序：**锁定并停写/停同步 → 备份 SQLite 一致快照与原 echo → 清点并保存证据 → 计算映射与缺口 → 回填提交 → 用空 DB 验证 → 发布完成标记 → 恢复正常写入**。必须在 continuation/scan 改写现存数据库前捕获救援快照；按库逻辑 ID/实际目录关联隔离其他库，不能把多根 DB 全量灌入当前库。先恢复已存在的 v1→v2 journal，再进行本次升级。

原 SQLite 在线备份仅保存在应用私有救援位置，**不放进音乐库、不作为目录恢复依赖**。`echo/migrations/` 的救援快照是从一致 DB 快照导出的目标库 typed 逻辑对象/事件证据及资源声明，移除绝对路径、端点、凭据、无关库与缓存，使用相对路径/逻辑 ID，并对 schema/count/digest 刷盘校验；导出完成前不触碰原库。目录有这份完整逻辑救援快照后，即使原 DB 和应用私有备份消失，仍能继续回填。用户封面字节必须复制并验证到目录，逻辑 snapshot 不能仅保留旧应用 cache key。

| 存量情况 | 决策 |
| --- | --- |
| DB 有 Song，目录完全无 Song | 保存该原 UUID、路径、哈希、added_at 以及真实喜欢/歌单/成员；媒体缺失也可写 missing 对象，不抛弃关系 |
| portable A 与 DB B 指向相同内容 | 优先可信 confirmed canonical/typed alias，其次已有可验证 portable 身份；按同库、相对路径和 BLAKE3 唯一映射，将 B 的关联投影到规范身份；记录本地迁移映射和旧引用，不能仅因 B 创建较晚而覆盖 A |
| 多身份同内容、墓碑或不同 binding 竞争 | 不靠路径/音乐键自动判定；检查确认版本、typed tombstone、alias 与原始事件。无法证明规范身份则阻止该分量自动迁移，保留全部证据并报告冲突 |
| 有引用到 A 的 Favorite，但无 A 的 Song/alias/可信账本 | 即使能把其他 DB 歌曲写成 B，也不能知道 A 属于谁；保留原记录，报告 unresolved，不按“唯一一首没喜欢”推断 |
| 无 PlaylistRecord，但 DB 有完整定义 | 用 DB 的原 playlist UUID/name/time 和成员回填；不能仅回填成员 |
| DB 与文件均无歌单名/关系 | 不凭 member 推测名称，不生成“恢复的歌单”冒充原数据；报告无法恢复 |
| DB 与 portable 同 UUID 业务值不同 | 默认已提交 portable 值；仅有可信未落盘事件/版本证明才可救援 DB 值，不能比较 mtime 或墙钟猜新旧；无证据时保留双方供显式选择 |
| 原缓存里有自定义封面 | 迁入目录 assets 后提交引用；缓存已丢则报告资源缺失，不能把损坏 cache key 当作成功 |

本地迁移映射不是主机确认 alias，未获确认不得写成 `PortableSyncMetadata.aliases`；不能伪造 authority/version。已发送 mutation 正文保留，不能因回填重写已发事件。无确认元信息的历史本地对象保持 local-only；若要恢复网络会话，另按现有 rebind/epoch 机制建立可信基线。

回填报告按种类给出发现、原有完整、补齐、迁移映射、缺媒体/资源、歧义、损坏、未关联数量与原因，并保留脱敏相对标识。以 Issue #72 计数搭建**合成 fixture**；不得宣称 11 条 favorite 必然都可恢复。无法关联只让恢复标记为 partial，阻止相关写入与“整库可恢复”完成标记，已验证的独立分量可以只读浏览。提供重试及经明确选择的修复入口，不悄悄丢弃记录。

### D7. 重建与错误反馈

打开顺序固定：锁/格式验证 → 迁移恢复与救援快照 → 已提交 journal 重放/清点 → typed envelope/alias/tombstone/confirmed base + pending 依赖解析 → 构建新的 SQLite 临时投影 → 对账 → 扫描关联媒体 → 原子切换活动投影。先建立全部身份再应用关系，dangling 记录保留到报告，不直接丢弃成 invalid 后永远遗忘。活动库切换失败维持旧库与其会话，候选库不能污染旧投影。

**悬空引用不得使重建整体失败。** 投影 MUST 分两段：可解析的对象与关系按顺序入正表；无法解析父引用（缺 Song、缺歌单定义、缺成员）的关系 MUST 保留原件、计入 partial 报告，而不是让单个 `UPDATE`/`INSERT` 撞外键或触发器导致整个事务回退、重建永久卡在待投影。已知悬空项须可从 `library_recovery_records`（按稳定 typed key 读回，不能用 Debug 格式做键）与原件重新尝试；补齐依赖后重试应自动关联，无需用户重新导入。

未知未来格式不改写，损坏对象保留原件并报告 partial；完整恢复需对象及依赖校验均通过。控制面只读时浏览既有可读提交（含只读卷上不取写锁），不产生新 UUID、不写喜欢/歌单/统计/同步对象；不会因为 media 可写就允许 DB-only 修改。SQLite 损坏时先隔离再重建，不因库字段完整就自行删除旧救援文件。

**导入的两段操作身份。** 一次导入包含“预留 claim”与“提交 Song”两个目录操作，二者 MUST 可自识别并互相引用：claim 携带预留 UUID、目标、哈希与 `added_at`，Song 提交携带同一预留 UUID；重试分别查询各自的稳定操作身份，不得因第二次提交失败而重新分配 UUID。界面只展示一个用户可见的导入结果，内部两段不得被报告为两次导入。

IPC 用结构化 result 区分 `NotSaved`、`SavedPendingRecovery`、`SavedPendingProjection`、`Complete`、`RecoveryPartial`；现有 UI 乐观更新在未提交失败时回退，已提交待恢复/待投影不能回退为“未保存”。每个新增错误码 MUST 在前端有明确 message key 映射（不得让 key 字符串直接作为用户文案），并显式进入 `ErrorPolicy` 的重试判定表而非依赖默认值。设置中的资料库区域显示“已保存到音乐库/正在恢复/有未恢复资料/音乐库只读”，诊断明细使用现有错误/详情模式，短 toast 不承担唯一持久故障提示。播放统计失败不中断播放，但必须可追踪并可重试；统计的幂等事件身份存于目录 state，不进入共享对象。

### D8. 验证、规范与清理

`specs/library-durability` 定义 LD-01…LD-10 验收矩阵。测试比较用户状态的规范快照（对象 UUID 集合、相对路径/哈希、added_at、喜欢 true/false、歌单定义、成员 UUID/顺序/时间、资源哈希、覆盖、设备统计桶、typed 墓碑/alias、pending 候选），不只比较行数。统计/候选只重放一次；重建绝不生成等价新上传事件。

故障 fake 逐步拦截复制/资源校验、文件刷盘、目录刷盘、提交标记、材料化第 N 个对象、DB transaction/commit、游标、独立检查点、ACK 与 GC；集成测试用临时目录及独立全新 SQLite。三平台原生验证记录 fsync/rename/锁/只读卷/Unicode/Windows 播放占用的真实结果，不能由单元测试代替。

适用标准：§2.1 文件/trait 规模；§3.1–3.3 向内依赖/ports/state machine/UoW；§4 错误与阻塞任务生命周期；§5 顺序迁移/无覆盖/UUID/同步；§6 生成 IPC 与错误反馈；§8–§9 契约、架构和可追溯验证。执行命令列于 tasks，交付须包含 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、前端 format/lint/typecheck/test/build、governance、IPC 漂移与 `openspec validate --all --strict`。不新增网络依赖；如文件锁/平台刷盘需要依赖，实施先记录许可证/MSRV及三平台证明。

**场景登记与门禁。** 本 change 的 delta 会向主规格新增 requirement 与 scenario，而 `docs/traceability.md`、`scripts/verify/manifest.json` 与 `scripts/verify/scenario-commands.mjs` 是三方可执行验收的事实来源。同步时 MUST 遵守两条编号纪律：新增 requirement 一律追加到目标主规格末尾；新增 scenario 一律追加到所属 requirement 末尾。任何插入都会重排既有稳定 ID（例如 `PLL-R07-S05`、`LL-R02…`），并使既有验收命令指向错误场景。同步后 MUST 重新生成场景清单并跑对账、命令有效性与重复度棘轮，不得只跑 `openspec validate`。

## Risks / Trade-offs

- [格式升级后旧程序无法写入] → manifest v3 明确拒绝，保留升级备份与只读导出，不能让 v2 程序忽略新 journal。
- [目录提交增大 IO] → 文件准备并发、同一逻辑集合批处理、有界重放和安全压缩；不以延后唯一权威落盘换吞吐，测量扫描/批量导入和同步空闲 CPU 回归。
- [主确认与独立检查点的顺序复杂] → 连续 published fence + 同 mutation receipt + 故障矩阵；缺失可信账本停止原 epoch，而本地用户资料仍可恢复。
- [介质不支持可靠刷盘/原子替换或突然拔盘] → 明确能力与错误；不宣称当前结果已可靠保存；保留 journal 与稳定 operation ID 供重试。
- [历史关系无证据] → 保留未关联记录和救援快照；恢复不完整如实呈现，不能臆造。
- [多个副本离线编辑后手工覆盖目录] → 普通目录复制不等于版本合并；禁止无证明覆盖已提交资料，继续使用既有同步冲突入口。
- [既有 UI change 同时增加歌单封面/排序字段] → 共享 DTO/资源契约串行合入；本次不重做界面布局，新增用户字段必须有 round-trip 测试。

## Migration Plan

1. 先实现读新协议、崩溃恢复和旧库清点测试，再切换任一生产写入口；所有入口接通之前不宣称完成 library-first。
2. 增加 SQLite 新顺序迁移、可恢复目录升级 journal；先发布旧程序不可写的 manifest 围栏，保留 v1/v2 原件与 SQLite 救援快照。
3. 执行 D6 回填；使用临时空 SQLite 按 D7 重建并比对规范快照，报告 partial 则保持只读且不得标记完整。验证成功后刷盘正式 v3 manifest/迁移完成标记。
4. 正常重启恢复有效本机会话；全新安装无凭据时先提供完全离线曲库，不自动联网、不伪造主权限。主机账本不可验证时按既有流程显式建立新 epoch；未确认本地内容保留供对账。
5. 回滚只允许恢复**整套升级前备份到独立目录**并验证；不能降版本号后用旧程序打开已经接受 v3 写入的库。已有 v3 用户数据可由支持版本导出，不能静默丢弃。
6. 实现完成并通过全部规定验证后，同步 delta specs 到主规格、复核文档、归档 change，再进入 PR 流程。本次规划不执行这些实施步骤。
