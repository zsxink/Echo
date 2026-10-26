# 本地资料库提交协议

本说明对应 `enforce-library-first-durability` 的目录协议与序列化契约，供实现、
诊断和恢复工具核对。字段定义在 Core Domain，刷盘和路径约束由 filesystem
adapter 执行，Application 协调提交与投影。资料库备份同时保留 `media/` 和整个
`echo/`；网络不传送目录私有日志。恢复步骤见 [恢复说明](lan-sync-recovery.md)。

## 版本分别解释

| 层次 | 当前读写契约 | 边界 |
| --- | --- | --- |
| 本地 manifest | 当前新建写 v3；兼容读取 v1/v2/v3。v3 可被当前客户端识别，v1/v2 只按旧格式读取 | manifest 版本决定目录写入能力；已发布的 v3 拒绝旧客户端（只认识 v1/v2）的写入。读取旧 manifest 不自动改写版本或迁移目录 |
| 对象文件 | 历史 v1 为带 `type` 的扁平记录；v2 为 `format_version: 2`、`record`、可选 `sync` | 读取支持的 v1/v2；v2 envelope 对未知版本拒绝，不回退为 v1。manifest 与 envelope 是独立版本轴 |
| 目录提交 | `LibraryCommit` 的字段和规范摘要；marker、材料化 checkpoint、coverage wrapper 的 `version` 均为 1 | prepared 正文本身没有独立 `version` 字段；不能从 marker 的 1 推断 manifest 是 v1 |
| 私有 state | 外层 `kind/id/payload` 及可选 `key`；正文用 `schema` 等标识版本 | 尚未覆盖所有历史正文的语义校验；未识别的历史证据不得自行授权恢复网络 |
| LAN wire | `MutationDto` / `CommitDto` 使用独立 `protocol_version: 1`；资源声明受长度、数量、摘要、MIME 和相对路径校验 | LAN 版本与本地 manifest/envelope 无关。目录专属资源位置不进入 wire；wire 已定义字段按既有 resource-ref 校验。serde 忽略未知 JSON 字段，因此不能用新增 wire 字段承载必须被旧端理解的语义。改变 wire 语义时必须升协议版本并拒绝不支持版本 |

### 独立兼容矩阵

| 输入 | 当前行为 | 写入/迁移行为 | 证据 |
| --- | --- | --- | --- |
| manifest v1/v2 | 当前客户端识别并按旧布局读取 | 读取不会自动升级版本；新建目录使用 v3 | `manifest_v3_explicitly_reads_legacy_but_refuses_invalid_and_future_versions` |
| manifest v3 | 当前客户端识别 | 旧客户端只接受 v1/v2，因而不能以旧写入器修改 v3 目录；不要求升级或回填既有库 | `library_durability_new_manifest_fences_legacy_writers_without_changing_record_envelopes` |
| record flat v1 / envelope v2 | 按各自 decoder 读取；v2 保持自己的 envelope 版本 | 目录读取和兼容投影不重写旧对象；v2 未知版本拒绝且不降级解释为 v1 | `portable_v` 与 `library_durability` 序列化/未知版本测试 |
| LAN wire v1 | 只有版本 1 被接受；mutation/commit 摘要由 wire DTO 计算 | 不从本地版本推断 LAN 能力。未知可选 JSON 字段被 serde 忽略；资源引用只能通过现有字段及校验规则交换。需要强制双方理解的新资源语义时必须升 wire 版本 | `echo-sync` protocol validation 与 server/client protocol tests |

这些版本轴不能互相代替：本地目录资源的 owner/version/location 是恢复证据，不能塞入 LAN payload 后假定旧端会保留；LAN resource-ref 仅交换既有媒体声明及相对路径。当前协议没有引入新 wire resource 字段，因此没有额外的跨版本字段可被静默丢弃。未来新增必需语义应使用显式能力/版本协商，而不是依赖 serde 未知字段处理。

## 提交及检查点字段

`echo/transactions/<operation-id>/prepared.json` 保存完整 `LibraryCommit`：

| 字段 | 语义 |
| --- | --- |
| `batch.operation_id` | 本地稳定操作 UUID；结果不确定时查询同一 ID，相同 ID 不允许不同 batch |
| `batch.library_id` | 本地目录 manifest 的 ID；与 LAN `SyncBinding.library_id` 属于不同身份域 |
| `batch.origin` | `local_edit/import/scan/remote/host_confirmation/migration/recovery`，防止重放被当作新编辑 |
| `batch.records` | 完整对象 after-images；同一 typed object 在集合内只出现一次 |
| `batch.state` | 私有恢复事实，与共享对象分开保存；缺字段读作空集合 |
| `batch.resources` | 已发布资源的 `relative_path/content_hash/byte_length`；adapter 在提交前重验实际字节 |
| `batch.expected` | typed `kind/object_uuid/content_hash` 前置条件；`content_hash: null` 要求对象原先不存在 |
| `library_commit_seq` | 本库正整数顺序，在目录写许可内分配，独立于 LAN `commit_seq` 与 SQLite |
| `predecessor_digest` | 首条为 null；后续绑定本地前驱真实摘要 |
| `digest` | BLAKE3，输入为 `PortableSerialize` 对 `(batch, seq, predecessor)` 输出的紧凑 JSON 字节 |

这里的规范 JSON 是当前 Rust 类型的确定性 serde 序列化，并非另一个通用 JSON
规范化标准。原 mutation 的 `original_body/original_resources_body` 是字符串，
即使空白不同也保留原文；不能先解析再重排它们或改写历史 journal 的摘要。

`committed.json` 包含 `version: 1`、`seq`、`digest`，必须与正文一致；prepared
单独存在仍未提交。可靠提交要求文件及必要父目录同步，不以文件已经可见为充分
证明。已发布 marker 后的失败保留同一操作并进入待恢复，不能按未保存重新分配 ID。

| 文件 | 字段及用途 |
| --- | --- |
| `echo/state/materialized-checkpoint.json` | `version/seq/head/files`；files 是材料化相对路径到实际内容 BLAKE3 的映射，只记录目录材料化进度 |
| `echo/state/coverage-checkpoint.json` | `version/image/digest`；image 含 `library_id/library_commit_seq/head_digest/records/state/resources`，digest 校验整个 image |
| `echo/state/operations/<uuid前两字符>/<operation-id>.json` | 被覆盖操作的不可变原 `LibraryCommit`，为按 ID 重试保留完整正文 |

覆盖快照必须先可靠刷盘，才能回收其覆盖的 journal；SQLite 水位不授权此回收。
当前每 32 个活跃提交压缩，最多容纳 256 个事务目录，单份提交/覆盖文件受 128 MiB
读取上限约束，超限时在写 marker 之前拒绝并保留链头。超过上限的写入不是"稍后
重试即可"：调用方必须让现有活跃后缀完成压缩或修复，失败会明确报告而不是静默
丢弃旧 journal。

回收顺序固定为：刷盘覆盖根与全部引用叶 → 逐份重刷不可变 receipt 与祖先目录 →
最后才删除 `prepared.json`、`committed.json` 和空事务目录。任一步中断后重启都
从锚点重做；GC 只删 journal 文件与过期快照叶，**不删除任何 `echo/media/`、
`echo/assets/`、`echo/tmp/` 下的字节**。因此候选、冲突、删除意图、未完成发布与
受管媒体版本天然受引用图保护；它们的声明同时存在于覆盖镜像与不可变 receipt 中，
即使 journal 已回收，全新 SQLite 也能从 `resources` 恢复 `relative_path/
content_hash/byte_length`。`echo/tmp/` 下的字节虽然保留，但永远不会被声明为
资源，因而不进入扫描或恢复候选。遇到坏摘要、断链、同序不同摘要或归属不符时
保留原件并拒绝写入。

## 资源声明与原件

`batch.resources` 的每一项描述一个**已经发布在资料库里的资源**，adapter 在写
marker 之前按 `relative_path` 打开文件，逐项核对长度与 BLAKE3，任何不符即整批
拒绝。此时还没有 prepared 正文、marker 或操作状态，调用方只能看到明确失败，
不会得到"已保存待恢复"。

只有两类位置可以被声明：

| 类别 | 路径 | 约束 |
| --- | --- | --- |
| 用户资源原件 | `echo/assets/<blake3>` | 路径必须逐字等于 `echo/assets/<content_hash>`，地址即摘要 |
| 受管媒体版本 | `echo/media/<kind>/<owner-uuid>/<declaration-digest>/<logical-path>` | 见下节 publication 的 owner/声明摘要校验 |

`echo/state/`、`echo/records/` 等控制面文件与 `echo/tmp/` staging 永远不是资源来源；
`content_hash` 必须是 64 位小写十六进制。受管版本缺失时报告打开失败，不创建占位
文件；原件被外部改写时提交与读取都报冲突，且**保留被发现时的字节**作为诊断证据
——Echo 不会为了符合声明而重写或"修复"任何媒体文件。

Echo 不写音乐文件标签：扫描与监听只读取字节并重新计算哈希，外部修改后的文件以
同一 `Song` UUID 与 `added_at` 重新提交，是本机可用性变化而不是源文件替换。

## typed 删除与别名

对象文件按 `records/<kind>/<uuid前两字符>/<uuid>.json` 分片。最终墓碑使用
`records/tombstones/<deleted-kind>/<uuid前两字符>/<uuid>.json`；正文携带
`object_uuid` 与 `deleted_kind`，对象键取被删除的真实类型。相同 UUID 的 song、
favorite 和 playlist-item 分别删除；不能按 UUID 单独覆盖另一类型，也不能把
本机 10 秒撤销意图当作最终墓碑。

`sync.aliases` 只有两种 typed 变体：`kind: song` 含
`alias_uuid/canonical_uuid`；`kind: playlist_item` 另外含 `playlist_uuid`。
alias 必须指向所在 envelope 的同类型规范对象，禁止自指、重复 alias 和其他
歌单的成员 alias。别名可随规范对象的最终墓碑保留；本地历史身份修复不因此成为
主机确认 alias。未知别名类型拒绝，不能只拿 UUID 跨类型推断。

## 私有事实、命名空间与冻结正文

state 物化于 `echo/state/<kind目录>/<id>.json`，例如 `playback-events`、
`confirmed`、`pending`、`pending-dispositions`、`receipts` 和 `sync-ledger`。
共享 `PlayStatsRecord` 只保存逐设备桶，播放事件身份留在私有 state。

新 scoped state 的 `key` 在 envelope 和 `payload.state_key` 内完全一致，包含：

| namespace 的 `scope` | 必须具备的原始归属 |
| --- | --- |
| `bound` | 完整 `binding`（同步库、authority、epoch） |
| `local_unbound` | 本地 `library_id` 和真实 `author`，只用于该作者的 mutation/disposition/local cursor |
| `legacy_evidence` | 仅 `source_root` 可证明；不能从当前配对追认历史 binding |

typed `target.fact` 区分 confirmed object、mutation、mutation_disposition、
receipt、ledger、song_alias、member_alias、origin_cursor、local_origin_cursor、
peer_cursor、conflict 和 publication。存储 `id` 来自带
`echo.durable-state-key.v1\0` 域分隔的完整 key 规范 JSON 的 BLAKE3 前 16 字节，
不等于原 LAN mutation ID。不同 binding/epoch 或正文版本的相同事件身份能共存。

`FrozenMutation` 保存 candidate 的事件/作者/序列/前驱、typed object、真实 base、
typed 资源，以及 operation、local revision、冻结毫秒、原 payload/resource JSON
字符串和原 payload digest。完整 frozen 正文摘要是 key 的一部分；相同完整 key
不得改正文。终结状态另写 `MutationDisposition`，通过 author、mutation ID 和
`frozen_body_digest` 引用原版本；迟到终结不得覆盖新版本。保留多个版本不代表可以
改写已经发送的网络正文，也不授权新作者重发旧事件。

confirmed 的完整 payload/版本、pending 正文、receipt 必须分别解释。当前
`records/` 用户值携带旧 sync metadata 不能单独证明这个新值已经主确认。
旧 `write_record` 入口仅允许幂等重写已确认的相同用户值；改变该值必须走保留
confirmed baseline 的 durable batch，不能复制旧确认到新值。旧入口与 envelope
入口在替换已有文件前均须成功解析原件；坏 JSON 或未知格式保留原字节并拒写。
本地 manifest ID 与 envelope 的同步库 ID 独立，不能靠 UUID 相等判断同步授权。
旧正文、缺 key 证据及不同绑定资料保留待核对；可信会话 projector、完整冻结写集
及显式新 binding adoption 仍在实施，离线恢复不创建凭据、监听或主权限。

## 各入口持久化与错误语义

所有用户变更都经 Core 的同一协调器（`plan_library_change`：目录提交在前、SQLite
投影在后），桌面与 IPC 层只做参数转发，不持有自己的写库路径。下表列出各入口的
持久化事实、失败语义和可重试方式；错误分类与消息键由 Core `Error::code()` 决定，
IPC 只映射为 `library.savedPendingRecovery` / `library.savedPendingProjection` 并
附带原 `operationId`。

| 入口 | 目录先提交的事实 | 失败语义 |
| --- | --- | --- |
| 导入 `ImportSongs` | 保留的 Song UUID、`added_at`、歌曲 after-image 与资源声明 | 目录未提交则不产生任何 SQLite 行；已提交但投影失败返回"已保存，待更新"，以同一 operation 重试 |
| 扫描 / 外部标签修改 / 移动 | 稳定 Song 身份、变更后的业务字段与新哈希 | 失败保持原 UUID 与 `added_at`；缺少媒体只改变本机可用性，不删除关系；Echo 不写源文件 |
| 喜欢 `SetFavorite` | true 与 false 两种状态（false 为显式事实） | 目录未提交不投影；已提交后重试同一操作不会复活旧 true |
| 歌单创建/改名/删除、成员增删 | 完整歌单定义、创建时间、成员身份/位置/加入时间与级联集合 | 空歌单、重复成员、顺序与级联删除都按提交顺序重建；删除与撤销见 `DeleteIntent` |
| 歌单/艺术家封面 | `echo/assets/<blake3>` 内容寻址原件 + 对象引用（长度、MIME） | 原件写入失败不改变已保存歌单/艺术家；读模型 cache key 由原件派生，清空缓存后可离线重建；只有旧 cache key 的历史封面显式报告"无可恢复原件" |
| 播放完成统计 | `StateKind::PlaybackEvent`（`echo/state/playback-events/<session>.json`）+ 逐设备桶的 `PlayStatsRecord` | 失败不打断播放；未保存的意图进入应用私有重试队列并可查询；重复 session、重放与重装不增计 |
| 删除 / 撤销 / 回收 | 本机 `DeleteIntent`、终态集合与资源声明 | 目录意图先于隐藏与 SQLite 删除；重启保持同一意图身份；未确认资源保留 retention hold |
| 启动续跑 `ContinueFromRecords` | 只读重放已提交链 | 控制面不可用时降级为只读，不伪造数据；部分恢复进入类型化未解析记录 |
| LAN 入站快照/增量 | 远端对象、候选、已应用水位与资源声明 | 快照分页失败不推进水位；对象先于媒体时保持不可用而非伪造成功 |
| 来源 ACK / 主机确认 | 完整 confirmed 正文、receipt、alias/墓碑与 publication | 未提交前不返回成功 ACK；ACK 后丢 SQLite 仍可恢复业务资料，但联网授权须另行验证账本 |

共享/同步正文永不携带播放会话或事件身份；该幂等事实只存在私有 state 目录。
LAN 的 receipt、凭据与主权限独立于可恢复用户资料。

### 与歌单封面 UI change 的字段交汇

`align-immersive-player-and-playlist-directory` 的 `playlist-management` 规格要求
歌单对话框提供封面预览、保存成功同步卡片与侧边导航、重命名不丢封面、失败保留
名称与已有封面，并明确不以按名称索引的 `localStorage` 模拟替代稳定歌单身份。
该规格与本 change 的交汇点只有一个字段：`PlaylistView.coverKey`。

- 来源：`coverKey` 由 `PortableRecord::Playlist.cover_asset`（内容寻址原件引用）
  派生，不是数据库里独立保存的用户值；清空应用缓存后由 `echo/assets/<hash>`
  重新生成同一个 key。
- 身份：封面挂在 `PlaylistId` 上，重命名、目录排序与拖动只改展示顺序，不触碰
  封面关联；成员变化不改封面。
- 失败：图片不是可用图片、读取失败或原件写入失败时返回错误，既有封面与名称
  保持不变；`hasCustomCover=false` 只表示用户清空，不表示原件丢失。
- 历史缺口：只有旧 cache key、没有原件的封面会明确报告"历史封面无可恢复原件"，
  UI 不得把它显示为已保存封面，也不得以破损图像替换。

## 完整确认与资源发布正文

`echo.sync.confirmed.v2` 要求 `author/mutation_id/version/sequence/payload/tombstone/changed_at_ms`、
原 LAN commit 字符串 `original_body` 及 `previous_digest/digest`。正文内的完整 binding、
对象、事件、base、origin 前驱、typed aliases 和资源声明保留原文；校验不能重编码
后代替原始摘要。LAN 资源的 `content_hash` 是小写十六进制，目录内 Domain manifest
的 `digest` 是 32 字节数组，读取时显式转换，不能把两种 JSON 形状混用。

导出仅按完整原 namespace 和 sequence 寻找真实 ledger；找不到或正文冲突时保留
`confirmed.v1`，不从当前对象或时钟补造确认。可信恢复仍要单独验证完整账本与实时
会话，不能从此正文授权联网。历史 v1 的显式 `payload:null` 墓碑须有匹配的已验证
ledger 删除事实；v2 使用 `{"tombstone":true}`。两者在 SQL baseline 中均写 NULL，
重复重试保留原 `changed_at_ms`，不能把普通 live null 当作删除。

`echo.sync.resource-publication.v2` 保存 `operation/publication/completed` 和 SQL journal
原始字符串 `original_body`。Domain 使用专用 DTO 校验，不能依赖 Application port：

- wrapper、原正文和 scoped key 的 operation/root/completed 必须一致；当前 SQL journal
  没有捕获历史 binding，namespace 仍为 `legacy_evidence`，不补造当前 binding。
- publication 的 typed object 是资源 owner；items 必须为 1–32 项，完整保留每项
  manifest（逻辑路径、digest、length、MIME）、替换前摘要、stage、staging/local/old 路径。
- 实际位置可等于安全逻辑路径，或为
  `echo/media/<kind>/<owner-uuid>/<declaration-digest>/<logical-path>`。
  declaration digest 是按既有 journal 字段顺序序列化全部 targets 后的 BLAKE3，
  表示资源声明集合版本，**不是主机 object_version**。旧位置须属于同一 owner 和
  逻辑路径，其旧声明摘要只能校验语法，不能从当前状态补造。
- staging 必须属于该 operation；路径须规范化且无逻辑/实际/staging 目的地碰撞；
  控制目录、其他 owner、错误声明版本、越界路径和无效 MIME 都拒绝。
- Completed 与全部 item stage 必须一致；预期旧摘要与旧位置须同时存在或同时为空。
  已声明完成仍只是历史凭据，恢复可播放状态还须独立校验文件与当前授权关联。

只有完整新导出通过 v2 校验才升级；旧布局或不完整 journal 继续导出为 v1 证据。
已提交目录中的 v1 正文和摘要不被改写。精确 v2 拒绝未知正文/嵌套字段，未来未知
schema 原样保留为待处理证据，不自动建立资源关联。

## 格式矩阵与历史缺省值

| 资料 | 当前验证边界 |
| --- | --- |
| flat v1 / envelope v2 | 完整用户字段、创建/加入时间、封面原件引用、typed 关系、false 喜欢、覆盖文本、逐设备桶经过 JSON 和目录 commit 往返 |
| 缺历史字段 | 继续保持 absent；不制造时间、资源或 sync 确认 |
| confirmed / receipt / frozen / publication | 分别版本化；完整原文与历史证据分开解释，不把当前用户值或资源拼成旧确认 |
| manifest v3 | 新建 v3 的识别、旧版本读取兼容及旧写入器围栏由独立测试覆盖；不要求把已存在的 v1/v2 目录改写为 v3 |
| 未知 envelope / manifest | 当前 envelope 版本 3 及更高版本拒绝解码；manifest 可读出版本但判不兼容、拒绝写入；不降级 v1 |

### 缺省字段

| 字段 | 读取和再次序列化 |
| --- | --- |
| 歌单 `created_at/cover_asset` | 缺失为 None，输出继续省略；不填当前时间或缓存 key |
| 成员 `added_at` | 缺失为 None，继续省略；`position` 才是成员顺序证据 |
| envelope `sync` | 缺失继续省略，不制造 binding/object version；空 aliases 继续省略 |
| state `key` | 缺失继续省略，旧 batch 的确定性 JSON 与既有 commit digest 不变 |
| 历史 Song `added_at` | 沿用已发布的整数 0 回退规则；0 只代表历史未知顺序，不是恢复时刻 |

不能把所有历史缺省一律变成 null、空对象或现在的时间。旧原件和 commit 按原
协议保留；本说明不承诺历史不存在的数据可推断出来。

序列化与故障回归入口：`cargo test -p echo-core library_durability --lib`，以及
`cargo test -p echo-core portable_v --lib`。测试覆盖固定旧摘要、缺字段、typed
墓碑/别名、跨 binding 版本、资源和目录链；它们不替代 Windows/Linux 原生认证、
完整 v3 升级和所有生产入口的顺序协调器验收。
