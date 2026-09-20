# 记录歌曲入库时间并随资料库恢复

## Why

歌曲的「添加到资料库的时间」是用户可见的产品事实（“最近添加”视图、`addedAt` 排序都依赖它），但当前该时间只存在于本机 SQLite `songs.added_at` 列中，**不进**可携带的资料库对象资料（`echo/records/songs/*.json`）。一旦本机数据库被清除、或换设备/移动资料库目录后从 `echo/` 恢复，重建的歌曲一律以 `Revision::INITIAL` 作为 `added_at`，所有歌曲的“添加时间”归零——`recent` 视图与 `addedAt` 排序随即退化为按 UUID 兜底，用户无法再看出任何“最近添加”顺序。产品需要把该时间随资料库一起记录与恢复。

## What Changes

- **可携带歌曲资料新增 `added_at` 字段**：`echo/records/songs/*.json` 中的歌曲对象记录携带入库时刻（墙钟毫秒，`added_at` 语义），随资料库同步/迁移。
- **恢复与接续时还原入库时间**：从资料库对象资料重建本地歌曲（恢复、以及打开已有控制面的资料库目录做接续投影）时，使用记录携带的 `added_at` 重建本地 `songs.added_at`，恢复后的“最近添加”与 `addedAt` 排序与源库一致。
- **向后兼容旧记录**：不含 `added_at` 字段的历史歌曲记录仍可被读取（缺省处理，选择无损兜底策略，见 design），不得破坏既有资料库打开与同步。
- **新建/更新的歌曲记录在写入时即带上该时间**：扫描新建、手动导入、`addedAt` 相关的现有写入路径，使写入 portable 记录与本地持久化使用一致的入库时刻。
- **BREAKING 无**：不改动 `0001` 已发布迁移，不改动本地排序/视图定义；本地 `added_at` 列语义保持不变。

范围仅 Core 数据层：桌面 UI 展示“x 天前加入”之类功能不在本 change 范围，另行立项。

## Capabilities

### New Capabilities

- 无（不引入新能力路径）

### Modified Capabilities

- `portable-library-layout`:歌曲可携带对象资料的载荷要求（需求 2）扩展为包含“入库时刻（`added_at`）”，随时资料库恢复/接续的行为要求（需求 3、5）扩展为“恢复后保留该时间”；历史记录缺该字段时仍须兼容可读。

## Impact

**领域对象**
- `crates/echo-core/src/domain/library.rs`：`SongRecord` 新增 `added_at` 字段（含序列化）。

**可携带资料写入/读取**
- `crates/echo-core/src/application/portable_materialize.rs`：`song_record()` 构造时写入 `added_at`。
- `crates/echo-core/src/application/continuation.rs`：读取 `SongRecord` 并用其 `added_at` 重建本地歌曲（替换 `Song::new(...)` 路径）。
- `crates/echo-core/src/infrastructure/filesystem/control_plane.rs`：读取时兼容缺省字段的历史记录。

**测试影响**
- `SongRecord` 构造、序列化 round-trip、恢复/接续投影相关测试需覆盖新增字段与旧记录缺省兼容；`library.rs`、`portable_materialize.rs`、`restore.rs`/`continuation.rs` 的现有测试需同步。

**不改动**
- SQLite `0001` 迁移与 `songs.added_at` 列：保持不变。
- 桌面 `SongView` DTO / IPC 类型 / 前端：本期不暴露该字段。
- `library-experience` 的视图与排序需求文本：依赖值不变，无需改写；恢复正确性由本 change 的 Core 侧数据保证。