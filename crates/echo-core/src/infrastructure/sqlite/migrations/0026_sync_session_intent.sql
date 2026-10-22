-- Persist the user's lifecycle choice with each root-scoped sync context.
-- Existing bindings are deliberately stopped so migration never opens a
-- listener or connection without an explicit user action.
ALTER TABLE sync_binding ADD COLUMN session_intent TEXT NOT NULL DEFAULT 'stopped'
    CHECK(session_intent IN ('enabled','stopped','paused'));

ALTER TABLE archived_sync_binding ADD COLUMN session_intent TEXT NOT NULL DEFAULT 'stopped'
    CHECK(session_intent IN ('enabled','stopped','paused'));

-- Host-recovery archives are column-for-column copies of the live tables, so
-- they must track the added column or their archival INSERT would supply one
-- value more than the archive table has columns. A recovery archive keeps the
-- session stopped; the replacement epoch always needs an explicit start.
ALTER TABLE recovery_sync_binding ADD COLUMN session_intent TEXT NOT NULL DEFAULT 'stopped'
    CHECK(session_intent IN ('enabled','stopped','paused'));
