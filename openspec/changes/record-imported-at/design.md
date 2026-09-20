# Design — 记录歌曲入库时间并随资料库恢复

## Context

见 proposal.md「Why」。现状要点(已核实):

- 本地入库时刻已存在:`Song.added_at: u64`(墙钟毫秒)是领域实体的「stable insertion ordering key」(`crates/echo-core/src/domain/entities.rs`),SQLite `songs.added_at` 列在 `0001` 迁移中建立、写入(`statements.rs` upsert)且 `ON CONFLICT DO UPDATE` 不覆盖它——所以元数据/收藏/播放变更永不改写入库时刻。
- 该时间随恢复丢失:可携带 `SongRecord`(`crates/echo-core/src/domain/library.rs`)没有该字段;恢复/接续投影在 `crates/echo-core/src/application/continuation.rs` 用 `Song::new(...)`(等价 `added_at = 0` = `Revision::INITIAL`)重建本地歌曲。恢复后所有歌曲 `addedAt` 归零,`recent`/`addedAt` 排序退化到 UUID 兜底。
- 恢复与接续共用同一投影实现:`restore.rs`(新设备)与根目录切换/打开资料库都委托 `ContinueFromRecords`(continuation),因此一处修复同时覆盖两条路径。
- 歌曲对象资料由 `portable_materialize.rs::song_record(device, hlc, revision, &Song)` 构造,入参是完整的 `&Song` 实体——`added_at` 已经可得。
- 序列化:领域数据类型统一 `#[serde(rename_all = "snake_case")]`,`SongRecord` 未启用 `deny_unknown_fields`。

## Goals / Non-Goals

**Goals**
- 歌曲可携带对象资料携带入库时刻,值等于本机 `added_at`,且随元数据/收藏/播放更新而不变。
- 从资料库恢复、以及打开已有控制面的资料库目录做接续时,还原本地入库时刻,使「最近添加」与 `addedAt` 排序与源库一致。
- 历史记录(无该字段)可读、可恢复,行为与旧版本一致。
- 改动仅限 Core 数据层,不触碰 SQLite 迁移、桌面 DTO/IPC/前端。

**Non-Goals**
- 不新增「导入时间」与「扫描时间」的区分语义(用户已确认两者都记「添加到资料库的时间」)。
- 不在桌面 UI 暴露或展示该字段(另立项)。
- 不引入新的迁移版本,不动已发布的 `0001`。

## Decisions

### D1: 字段命名与类型 — `SongRecord::added_at: u64`

与领域实体、DB 列同名同类型,序列化后为 snake_case `added_at`。选择理由:
- 语义一致:它就是「添加到资料库的时间」,不是独立的「导入时间」。
- 写入处 `song_record()` 可直接 `song.added_at()` 复制,零换算。
- 其它 payload 字段(`media_path`、`content_hash`)同样是领域原样映射,风格统一。
- 备选:命名 `imported_at`/`addedAt`(camelCase)——会引入与既有 snake_case 布局不一致、且与领域语义偏离的别名,拒绝。

#### 序列化形状(实测,非推测)

磁盘路径(来自 `control_plane.rs` 的 `record_path`):

```
<资料库根目录>/echo/records/songs/<uuid 前两位 hex>/<歌曲 uuid>.json
```

实施前写出的记录(`SongRecord` 无 `added_at`,即“从资料库恢复丢时间”的现状)——由真实代码 `serde_json::to_string_pretty` 输出:

```json
{
  "type": "song",
  "song_uuid": "1f4b6a2c-8c3e-4d5a-9e21-0b6f2a3c4d5e",
  "revision": 7,
  "updated_by_device_id": "6f2a3c4d-5e6f-4a7b-8c9d-0a1b2c3d4e5f",
  "hlc": { "wall_secs": 1700000000, "counter": 2 },
  "media_path": { "normalized": "media/周杰伦/周杰伦 - 晴天.flac" },
  "content_hash": "d44d1c2f4a28d3b0e5a6b7c8d9e0f1a2b3c4d5e6f7a0b1c2d3e4f5a6b7c8d9e0",
  "title": "晴天",
  "artist": "周杰伦",
  "album": "叶惠美"
}
```

实施后(D1 + D2),同一记录只多一个整数毫秒时间戳,与 SQLite `songs.added_at` 同值:

```json
{
  "type": "song",
  "song_uuid": "1f4b6a2c-8c3e-4d5a-9e21-0b6f2a3c4d5e",
  "revision": 7,
  "updated_by_device_id": "6f2a3c4d-5e6f-4a7b-8c9d-0a1b2c3d4e5f",
  "hlc": { "wall_secs": 1700000000, "counter": 2 },
  "media_path": { "normalized": "media/周杰伦/周杰伦 - 晴天.flac" },
  "content_hash": "d44d1c2f4a28d3b0e5a6b7c8d9e0f1a2b3c4d5e6f7a0b1c2d3e4f5a6b7c8d9e0",
  "added_at": 1700000000123,
  "title": "晴天",
  "artist": "周杰伦",
  "album": "叶惠美"
}
```

关键序列化事实(实测):`hlc` 为展开对象(`wall_secs`+`counter`)、`media_path` 为 `{ "normalized": ... }` 对象、`revision`/`song_uuid`/`updated_by_device_id` 为字符串,顶层 `"type": "song"` 来自 `PortableRecord` 的 `#[serde(tag = "type")]`;`added_at` 是普通整数,无需自定义序列化。

### D2: 历史记录缺字段兼容 — `#[serde(default)]` 归零

`SongRecord::added_at` 声明为 `u64` 并加 `#[serde(default)]`:历史 JSON 缺该字段时反序列化为 `0`。

- 新 reader 读旧记录 → 缺省 `0`,与旧版本恢复时 `Song::new` 得到的 `added_at = 0` 逐位一致,「最近添加」兜底顺序不变。
- 旧 reader 读新记录 → 因无 `deny_unknown_fields`,未知字段 `added_at` 被忽略,记录仍可读。双向兼容,无需 manifest 版本跳升。
- 备选 `Option<u64>`:比 `u64 + default` 多一层分支,而任何缺省最终都落回既有兜底(0);`Option` 只会让连续性代码多处理一个 `None`。拒绝。

### D3: 不在本地数据层新增存储 — 复用既有 `songs.added_at`

`added_at` 本地已持久化并有索引(`songs_by_added`)。本 change 只解决「它不进可携带资料、恢复时不还原」两个缺口,不新建列/迁移。备选「新增独立导入时间列」:用户已明确不需要区分,范围外。

### D4: 还原点收敛到续接投影 — `continuation.rs` 的歌曲重建调用点

`restore.rs` 与接续两条路径都经 `ContinuationFromRecords`。把 `Song::new(id, root, path, Revision::INITIAL)` 改为 `Song::with_added_at(id, root, path, Revision::INITIAL, song_record.added_at)`(缺省时即 `0`)。

- 一处修改覆盖「从资料库恢复」与「打开既有控制面资料库」两条 spec 场景路径。
- 投影后 `media/` 扫描的 relink/revival 只回填媒体状态,不触碰 `added_at`(既有行为),因此扫描不会改写还原回来的时间。

### D5: content hash 会变,属预期演进

portable 记录的确定性 content hash 基于全量 JSON,新增字段会改变哈希。这是格式的顺向演进(旧 reader 兼容),不需要 bump manifest 格式版本,`snapshot.json` 可再生。同步载荷增量只有一个整数。

## Risks / Trade-offs

- [恢复后的「最近添加」对新增记录排序正确,但对缺字段的历史记录仍保持旧的 UUID 兜底] → 这是旧版本的既成行为,spec 明确「不伪造时间」;新写入的记录携带真值,资料库随时间自然补齐。
- [`SongRecord` 是公开 struct,所有结构化构造点需补 `added_at` 字段,属编译期强制] → 在 tasks 中枚举构造点(`library.rs` 测试、`restore.rs` 测试、号通用 `song_record` 测试等)逐一更新;编译失败即兜住遗漏。
- [portable content hash 相关断言(确定性、round-trip)会因字段变化而失败] → 同步更新会续期哨兵/期望值;e2e 用一个“老格式记录(无 added_at)”反序列化用例锁死兼容。
- [旧设备/旧版本 Echo 打开含新字段的资料库] → 无 `deny_unknown_fields`,静默忽略;不受影响。
- [写入路径遗漏:任何不走 `song_record()` 的歌曲记录生成点漏写字段] → `# [serde(default)]` 保证漏写不崩溃(值 0 兜底),且同步基础数据侧有 round-trip 测试;tasks 中明确要求用一次全仓 grep 挂历所有 `SongRecord {` 构造点。

## Migration Plan

- 无数据库迁移:本地列已存在。
- 资料库侧:旧 `echo/records/songs/*.json` 保持可读(缺字段→0);新写入叠加 `added_at`。无重写/回填动作。
- 回滚:字段为可选增量,回退代码版本即可,旧记录与新字段均兼容。

## Open Questions

- 暂无。已确认:时间语义复用 `added_at`;范围仅 Core 数据层;恢复还原点在续接投影。