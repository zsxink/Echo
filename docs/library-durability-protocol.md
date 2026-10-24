# 本地资料库提交协议

本说明对应 `enforce-library-first-durability` 的目录协议与序列化契约，供实现、
诊断和恢复工具核对。字段定义在 Core Domain，刷盘和路径约束由 filesystem
adapter 执行，Application 协调提交与投影。资料库备份同时保留 `media/` 和整个
`echo/`；网络不传送目录私有日志。恢复步骤见 [恢复说明](lan-sync-recovery.md)。

## 版本分别解释

| 层次 | 当前读写契约 | 边界 |
| --- | --- | --- |
| manifest | 当前写 2，读取兼容 1/2 | 目标 manifest 3 的旧客户端写入围栏及完整迁移尚未完成；不能手工改版本号冒充升级 |
| 对象文件 | 历史 v1 为带 `type` 的扁平记录；v2 为 `format_version: 2`、`record`、可选 `sync` | 声明未知 envelope 版本时拒绝，不能回退为 v1；manifest 与 envelope 的未来版本须分别协商 |
| 目录提交 | `LibraryCommit` 的字段和规范摘要；marker、材料化 checkpoint、coverage wrapper 的 `version` 均为 1 | prepared 正文本身没有独立 `version` 字段；不能从 marker 的 1 推断 manifest 是 v1 |
| 私有 state | 外层 `kind/id/payload` 及可选 `key`；正文用 `schema` 等标识版本 | 尚未覆盖所有历史正文的语义校验；未识别的历史证据不得自行授权恢复网络 |
| LAN wire | 独立的协议版本与能力校验 | 不从本地格式版本推断对端能接收新字段；混版本资源协商仍属专门验收 |

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
读取上限约束，超限失败。活跃后缀有界不等于整个历史 state/资源引用图已实现有界
回收，后者仍未完成。遇到坏摘要、断链、同序不同摘要或归属不符时保留原件并拒绝写入。

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
旧正文、缺 key 证据及不同绑定资料保留待核对；可信会话 projector、完整冻结写集
及显式新 binding adoption 仍在实施，离线恢复不创建凭据、监听或主权限。

## 历史缺省值

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
