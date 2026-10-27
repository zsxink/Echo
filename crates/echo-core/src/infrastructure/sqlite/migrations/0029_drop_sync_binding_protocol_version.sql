-- Retire the per-binding protocol_version column. Every binding has carried
-- value 1 since LAN sync shipped; the wire protocol version lives on the
-- mutation envelope instead, so the column was dead weight on every insert.
--
-- archived_sync_binding and recovery_sync_binding are column-for-column
-- copies of the live table filled by `SELECT live.*` (see 0026 for the same
-- pairing), so the retired column must leave them in the same migration or
-- every archival INSERT would supply one value fewer than the copy has.
ALTER TABLE sync_binding DROP COLUMN protocol_version;
ALTER TABLE archived_sync_binding DROP COLUMN protocol_version;
ALTER TABLE recovery_sync_binding DROP COLUMN protocol_version;
