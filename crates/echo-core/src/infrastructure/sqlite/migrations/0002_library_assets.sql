-- 0002: library asset references (phase 4: media parsing, scan and watching).
--
-- The scan pipeline (task 4.6) must keep the cover cache key and the database
-- reference transactionally consistent, so each song row carries the content
-- hash of the cover asset it references. The reference is nullable: songs
-- without cover art (and rows written before this migration) stay NULL.
--
-- The runtime-state table persists the monotonic `root_epoch` that the root
-- switch barrier advances inside the same SQLite transaction that flips the
-- active root (design §5, `CommitActivation`). One row, `key = 'root_epoch'`.

ALTER TABLE songs ADD COLUMN cover_hash TEXT REFERENCES cover_assets(content_hash);

CREATE TABLE runtime_state (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
