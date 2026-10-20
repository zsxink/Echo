-- Retained media does not depend on the lifetime of a catalog object.
CREATE TABLE sync_retained_resources (
 local_root_uuid TEXT NOT NULL, content_hash BLOB NOT NULL CHECK(length(content_hash)=32),
 byte_length INTEGER NOT NULL CHECK(byte_length>=0), local_relative_path TEXT NOT NULL,
 PRIMARY KEY(local_root_uuid,content_hash)
);
-- A rebuild is committed atomically. Cleanup must fail if any owner is corrupt.
CREATE TABLE sync_resource_holds (
 local_root_uuid TEXT NOT NULL, owner_kind TEXT NOT NULL, owner_id TEXT NOT NULL,
 content_hash BLOB NOT NULL CHECK(length(content_hash)=32),
 PRIMARY KEY(local_root_uuid,owner_kind,owner_id,content_hash)
);
CREATE INDEX sync_resource_holds_hash ON sync_resource_holds(local_root_uuid,content_hash);
CREATE TABLE sync_remote_delete_isolations (
 local_root_uuid TEXT NOT NULL, object_uuid TEXT NOT NULL, object_version INTEGER NOT NULL,
 operation_uuid TEXT NOT NULL UNIQUE, source_relative_path TEXT NOT NULL,
 content_hash TEXT NOT NULL, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN(0,1)),
 PRIMARY KEY(local_root_uuid,object_uuid,object_version)
);
