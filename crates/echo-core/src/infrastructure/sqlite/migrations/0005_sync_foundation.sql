-- Echo 0.1.0 sync-foundation schema (方向 A: schema 骨架先于行为)。
--
-- 0.1.0 提供"同步基础数据底座"：让本地数据的形状从首日即可被二期同步
-- 引擎消费，但不提供任何可操作同步。此迁移只改变数据形状 —— 建表、加
-- revision 字段、写入一条 sync_state —— 不接线上传/下载/连接器/UI 入口。
--
-- revision 语义（全同步候选对象统一）：
--   单调递增的整体版本号，二期用于"最后写入者胜出"的冲突裁决。本迁移只
--   新增列并写入 0；0.1.0 应用层按 A1 预写（本地变更把 revision=0→1 递增）
--   但二期协议批准前不参与网络裁决。
--
-- tombstones / sync_outbox 语义（仅形状，无行为）：
--   删除与待上传队列的本地持久化。0.1.0 不写、不读这些表；二期以新迁移
--   按已批准同步协议落地行为。此迁移只是预先固定 schema 形状，避免二期
--   因对象 revision 缺失或表结构未定而返工。

ALTER TABLE song_overrides ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE library_roots ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;

-- 覆盖 layer 的版本化（song_overrides.revision 由本条 ALTER 提供）。

CREATE TABLE tombstones (
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    deleted_at INTEGER NOT NULL,
    PRIMARY KEY (object_type, object_uuid)
);

CREATE TABLE sync_state (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE sync_outbox (
    outbox_id INTEGER PRIMARY KEY AUTOINCREMENT,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    operation TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    pushed_at INTEGER
);
CREATE INDEX sync_outbox_pending ON sync_outbox(pushed_at IS NULL, created_at);
-- A revision is the object's monotone change ordinal (outbox history derived):
-- the same (object, revision) pair must never be written twice, so repeated /
-- recovered mutations become idempotent no-ops instead of duplicate rows.
CREATE UNIQUE INDEX sync_outbox_revision_once
    ON sync_outbox(object_type, object_uuid, revision);

-- 标记"数据结构已为同步就绪"，二期同步引擎以这条 state 判定无需回填历史。
INSERT INTO sync_state (key, value) VALUES ('schema_base', 'full');