-- Durable retry markers for retiring one binding's disposable transfer cache.
-- The marker is written before the binding identity is discarded, so a failed
-- directory removal can be retried by a later session without deleting local
-- music, journals or security checkpoints.
CREATE TABLE sync_cache_cleanups (
    binding_key TEXT PRIMARY KEY NOT NULL,
    binding_json TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
