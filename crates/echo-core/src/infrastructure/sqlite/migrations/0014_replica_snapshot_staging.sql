-- Enrollment data is invisible until its final page commits the projection.
CREATE TABLE sync_replica_snapshot_sessions (
    peer_device_id TEXT PRIMARY KEY NOT NULL,
    token TEXT NOT NULL UNIQUE,
    library_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    commit_seq INTEGER NOT NULL CHECK (commit_seq >= 0),
    commit_digest BLOB NOT NULL CHECK (length(commit_digest) = 32),
    next_page_index INTEGER NOT NULL CHECK (next_page_index >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
);
CREATE TABLE sync_replica_snapshot_pages (
    token TEXT NOT NULL REFERENCES sync_replica_snapshot_sessions(token) ON DELETE CASCADE,
    page_index INTEGER NOT NULL CHECK (page_index >= 0),
    body_digest BLOB NOT NULL CHECK (length(body_digest) = 32),
    body_json TEXT NOT NULL,
    PRIMARY KEY (token, page_index)
);
CREATE TABLE sync_replica_snapshot_records (
    token TEXT NOT NULL REFERENCES sync_replica_snapshot_sessions(token) ON DELETE CASCADE,
    identity TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (token, identity)
);

-- A digest may legitimately appear at several paths and in several roles.
-- sync_resources remains the local locator; these retain each shared manifest.
CREATE TABLE sync_resource_manifests (
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    ref_kind TEXT NOT NULL CHECK (ref_kind IN ('audio', 'lyrics_sidecar', 'user_cover', 'playlist_cover')),
    library_relative_path TEXT NOT NULL,
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    media_type TEXT NOT NULL,
    PRIMARY KEY (object_type, object_uuid, content_hash, ref_kind, library_relative_path)
);
