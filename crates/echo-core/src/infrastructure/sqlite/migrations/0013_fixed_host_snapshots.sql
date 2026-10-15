-- Frozen published object and alias pages. No active SQLite connection or
-- mutable local projection is retained by a snapshot session.
CREATE TABLE sync_snapshot_sessions (
    token TEXT PRIMARY KEY NOT NULL,
    library_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq >= 0),
    commit_digest BLOB NOT NULL CHECK (length(commit_digest) = 32),
    page_size INTEGER NOT NULL CHECK (page_size BETWEEN 1 AND 512),
    record_count INTEGER NOT NULL CHECK (record_count >= 0),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK (expires_at > created_at)
);
CREATE INDEX sync_snapshot_expiry ON sync_snapshot_sessions(expires_at);
CREATE TABLE sync_snapshot_records (
    token TEXT NOT NULL REFERENCES sync_snapshot_sessions(token) ON DELETE CASCADE,
    record_index INTEGER NOT NULL CHECK (record_index >= 0),
    record_json TEXT NOT NULL,
    PRIMARY KEY (token, record_index)
);
-- Media cleanup must treat these hashes as retained while the session lives.
CREATE TABLE sync_snapshot_resource_refs (
    token TEXT NOT NULL REFERENCES sync_snapshot_sessions(token) ON DELETE CASCADE,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    PRIMARY KEY (token, content_hash)
);
CREATE INDEX sync_snapshot_resource_hash ON sync_snapshot_resource_refs(content_hash);
