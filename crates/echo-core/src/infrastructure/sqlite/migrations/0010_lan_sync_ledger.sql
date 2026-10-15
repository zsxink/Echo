-- LAN synchronization durable ledger. Append-only migration: local databases
-- keep their existing rows, UUIDs and revision high-waters.

CREATE TABLE sync_binding (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    library_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    role TEXT NOT NULL CHECK (role IN ('host', 'replica')),
    protocol_version INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE sync_peers (
    device_id TEXT PRIMARY KEY NOT NULL,
    library_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    credential_handle TEXT NOT NULL,
    authorized_at INTEGER NOT NULL,
    revoked_at INTEGER
);

CREATE TABLE sync_local_highwater (
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    local_revision INTEGER NOT NULL CHECK (local_revision >= 0),
    PRIMARY KEY (object_type, object_uuid)
);
-- Preserve the latest revision even if an operator later archives/removes the
-- local outbox rows. New writes continue from this durable high-water.
INSERT INTO sync_local_highwater (object_type, object_uuid, local_revision)
SELECT object_type, object_uuid, MAX(revision)
FROM sync_outbox
GROUP BY object_type, object_uuid;

CREATE TABLE sync_origin_cursors (
    origin_device_id TEXT PRIMARY KEY NOT NULL,
    origin_seq INTEGER NOT NULL CHECK (origin_seq >= 0),
    last_mutation_id TEXT
);

CREATE TABLE sync_device_play_counts (
    song_uuid TEXT NOT NULL,
    device_id TEXT NOT NULL,
    play_count INTEGER NOT NULL CHECK (play_count >= 0),
    PRIMARY KEY (song_uuid, device_id)
);

ALTER TABLE sync_outbox ADD COLUMN mutation_id TEXT;
CREATE UNIQUE INDEX sync_outbox_mutation_id ON sync_outbox(mutation_id) WHERE mutation_id IS NOT NULL;

CREATE TABLE sync_mutations (
    mutation_id TEXT PRIMARY KEY NOT NULL,
    origin_device_id TEXT NOT NULL,
    origin_seq INTEGER NOT NULL CHECK (origin_seq > 0),
    previous_mutation_id TEXT,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    local_revision INTEGER NOT NULL CHECK (local_revision > 0),
    base_object_version INTEGER,
    payload_json TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'rejected')),
    outcome_json TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (origin_device_id, origin_seq)
);
CREATE INDEX sync_mutations_pending ON sync_mutations(status, origin_device_id, origin_seq);
CREATE INDEX sync_mutations_object ON sync_mutations(object_type, object_uuid, local_revision);

CREATE TABLE sync_inbox (
    origin_device_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    outcome_json TEXT NOT NULL,
    commit_seq INTEGER,
    received_at INTEGER NOT NULL,
    PRIMARY KEY (origin_device_id, mutation_id)
);

CREATE TABLE sync_commits (
    commit_seq INTEGER PRIMARY KEY CHECK (commit_seq > 0),
    library_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    origin_device_id TEXT NOT NULL,
    mutation_id TEXT NOT NULL,
    canonical_json TEXT NOT NULL,
    previous_digest BLOB NOT NULL CHECK (length(previous_digest) = 32),
    commit_digest BLOB NOT NULL UNIQUE CHECK (length(commit_digest) = 32),
    publication_state TEXT NOT NULL CHECK (publication_state IN ('pending', 'published')),
    created_at INTEGER NOT NULL,
    UNIQUE (origin_device_id, mutation_id)
);
CREATE INDEX sync_commits_publication ON sync_commits(publication_state, commit_seq);

CREATE TABLE sync_object_versions (
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    object_version INTEGER NOT NULL CHECK (object_version > 0),
    commit_seq INTEGER NOT NULL,
    payload_json TEXT,
    is_tombstone INTEGER NOT NULL DEFAULT 0 CHECK (is_tombstone IN (0, 1)),
    PRIMARY KEY (object_type, object_uuid)
);

CREATE TABLE sync_song_aliases (
    alias_song_uuid TEXT PRIMARY KEY NOT NULL,
    canonical_song_uuid TEXT NOT NULL,
    commit_seq INTEGER NOT NULL,
    CHECK (alias_song_uuid <> canonical_song_uuid)
);

CREATE TABLE sync_member_aliases (
    playlist_uuid TEXT NOT NULL,
    alias_member_uuid TEXT NOT NULL,
    canonical_member_uuid TEXT NOT NULL,
    commit_seq INTEGER NOT NULL,
    CHECK (alias_member_uuid <> canonical_member_uuid),
    PRIMARY KEY (playlist_uuid, alias_member_uuid)
);

CREATE TABLE sync_conflicts (
    conflict_id TEXT PRIMARY KEY NOT NULL,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    replaced_version INTEGER NOT NULL,
    accepted_version INTEGER NOT NULL,
    replaced_payload_json TEXT NOT NULL,
    accepted_payload_json TEXT NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX sync_conflicts_open ON sync_conflicts(resolved_at, object_type, object_uuid);

CREATE TABLE sync_peer_cursors (
    peer_device_id TEXT PRIMARY KEY NOT NULL,
    applied_commit_seq INTEGER NOT NULL DEFAULT 0 CHECK (applied_commit_seq >= 0),
    applied_digest BLOB NOT NULL CHECK (length(applied_digest) = 32),
    resource_bytes_complete INTEGER NOT NULL DEFAULT 0 CHECK (resource_bytes_complete >= 0),
    updated_at INTEGER NOT NULL
);

CREATE TABLE sync_local_delete_intents (
    intent_id TEXT PRIMARY KEY NOT NULL,
    final_mutation_id TEXT NOT NULL UNIQUE,
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    undo_deadline INTEGER NOT NULL CHECK (undo_deadline >= created_at),
    state TEXT NOT NULL CHECK (state IN ('open', 'cancelled', 'finalized', 'remote_deleted')),
    finalized_at INTEGER
);
CREATE INDEX sync_delete_intents_open ON sync_local_delete_intents(state, undo_deadline);

CREATE TABLE sync_transfers (
    peer_device_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('upload', 'download')),
    expected_length INTEGER NOT NULL CHECK (expected_length >= 0),
    completed_length INTEGER NOT NULL DEFAULT 0 CHECK (completed_length >= 0 AND completed_length <= expected_length),
    state TEXT NOT NULL CHECK (state IN ('staging', 'verified', 'published', 'failed')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (peer_device_id, content_hash, direction)
);

CREATE TABLE sync_resources (
    content_hash BLOB PRIMARY KEY NOT NULL CHECK (length(content_hash) = 32),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('audio', 'lyrics_sidecar', 'user_cover', 'playlist_cover')),
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    media_type TEXT NOT NULL,
    library_relative_path TEXT NOT NULL,
    local_state TEXT NOT NULL CHECK (local_state IN ('missing', 'staging', 'verified', 'available', 'conflict'))
);

CREATE TABLE sync_resource_refs (
    object_type TEXT NOT NULL,
    object_uuid TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    ref_kind TEXT NOT NULL CHECK (ref_kind IN ('audio', 'lyrics_sidecar', 'user_cover', 'playlist_cover')),
    PRIMARY KEY (object_type, object_uuid, content_hash, ref_kind)
);
CREATE INDEX sync_resource_refs_hash ON sync_resource_refs(content_hash);
