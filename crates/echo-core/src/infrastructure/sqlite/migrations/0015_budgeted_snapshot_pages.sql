-- Legacy record-count pages can exceed the receiver's combined record/resource
-- or JSON budgets. Invalidate only these temporary host sessions; enrollment
-- can capture a fresh session. Library projections and replica journals remain.
DELETE FROM sync_snapshot_resource_refs;
DELETE FROM sync_snapshot_records;
DELETE FROM sync_snapshot_sessions;

CREATE TABLE sync_snapshot_pages (
    token TEXT NOT NULL REFERENCES sync_snapshot_sessions(token) ON DELETE CASCADE,
    page_index INTEGER NOT NULL CHECK (page_index >= 0),
    record_offset INTEGER NOT NULL CHECK (record_offset >= 0),
    record_count INTEGER NOT NULL CHECK (record_count BETWEEN 0 AND 512),
    PRIMARY KEY (token, page_index)
);
