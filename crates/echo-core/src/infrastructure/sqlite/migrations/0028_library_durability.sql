-- SQLite records only the applied prefix of the library-owned commit chain.
CREATE TABLE library_projection_watermarks (
    library_root_uuid TEXT PRIMARY KEY NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    library_id TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK(commit_seq >= 0),
    commit_digest TEXT
);
CREATE TABLE library_applied_operations (
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    operation_uuid TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK(commit_seq > 0),
    commit_digest TEXT NOT NULL,
    PRIMARY KEY(library_root_uuid, operation_uuid),
    UNIQUE(library_root_uuid, commit_seq)
);
CREATE TABLE library_recovery_records (
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    PRIMARY KEY(library_root_uuid, object_key)
);
-- Preserve complete typed facts even when their live projection awaits a parent.
ALTER TABLE library_recovery_records ADD COLUMN row_kind TEXT NOT NULL DEFAULT 'record'
    CHECK(row_kind IN ('record','state','resource'));
ALTER TABLE library_recovery_records ADD COLUMN unresolved_reason TEXT;
