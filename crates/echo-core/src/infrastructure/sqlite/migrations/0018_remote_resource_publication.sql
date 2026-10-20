-- Keep object/media completion separate. Old files and staging paths remain
-- retained while publication is interrupted or conflicts with foreign data.
CREATE TABLE sync_remote_publications (
    operation_uuid TEXT PRIMARY KEY NOT NULL,
    local_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    declaration_json TEXT NOT NULL,
    publication_json TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1))
);
CREATE TABLE sync_remote_publication_claims (
    operation_uuid TEXT NOT NULL REFERENCES sync_remote_publications(operation_uuid) ON DELETE RESTRICT,
    local_root_uuid TEXT NOT NULL,
    normalized_relative_path TEXT NOT NULL,
    released INTEGER NOT NULL DEFAULT 0 CHECK(released IN (0,1)),
    PRIMARY KEY(operation_uuid, normalized_relative_path)
);
CREATE UNIQUE INDEX sync_remote_publication_active_claim
    ON sync_remote_publication_claims(local_root_uuid, normalized_relative_path) WHERE released=0;
CREATE TABLE sync_remote_resource_locations (
    local_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK(length(content_hash)=32),
    library_relative_path TEXT NOT NULL,
    local_relative_path TEXT NOT NULL,
    operation_uuid TEXT NOT NULL REFERENCES sync_remote_publications(operation_uuid) ON DELETE RESTRICT,
    PRIMARY KEY(local_root_uuid,object_type,object_uuid,library_relative_path)
);
