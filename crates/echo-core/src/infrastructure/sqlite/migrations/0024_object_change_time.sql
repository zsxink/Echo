-- Conflict arbitration compares the change time frozen by the mutation, not a
-- value carried inside the payload. Modern payloads never embed an HLC, so the
-- payload-derived time was zero on both sides and the host's existing value
-- silently won every update. Persist the winning edit time with the confirmed
-- object so the next candidate can be compared against it.
ALTER TABLE sync_object_versions ADD COLUMN changed_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE archived_sync_object_versions ADD COLUMN changed_at_ms INTEGER NOT NULL DEFAULT 0;
-- Recovery archives are `CREATE TABLE ... AS SELECT` clones of the live rows,
-- so they carry the live column list and must gain the new column too.
ALTER TABLE recovery_sync_object_versions ADD COLUMN changed_at_ms INTEGER NOT NULL DEFAULT 0;
