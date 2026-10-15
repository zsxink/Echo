-- The marker and all initial snapshots are committed in one writer transaction.
-- Reopening or retrying host startup must never enqueue another baseline.
CREATE TABLE sync_host_baselines (
    library_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    local_root_uuid TEXT NOT NULL,
    object_count INTEGER NOT NULL CHECK (object_count >= 0),
    completed_at INTEGER NOT NULL,
    PRIMARY KEY (library_id, authority_id, epoch)
);
