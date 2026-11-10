-- The retained catalog holds one row per projected object, so it grows with
-- the library. Every projection pass used to walk that whole table, three
-- times over:
--
--   * the unresolved-row scan looked for `unresolved_reason IS NOT NULL`
--     across every row;
--   * the standalone-root cleanup deleted detached protocol states with a
--     `row_kind='state'` filter plus two `json_extract` predicates, which
--     still had to read every row of the table to find the few state rows;
--   * the ordinary read decoded every retained record's JSON, even though a
--     settled catalog has nothing to retry.
--
-- These three indexes answer exactly those questions without touching the
-- rows they do not need. All three are partial or composite on purpose: a
-- settled, standalone library keeps them nearly empty, so a projection pass
-- costs what the pending work costs rather than what the library size costs.
--
-- The unresolved index covers only the rows that still carry a reason: a
-- partially-satisfied dependency is the normal state for a retained fact
-- whose parent has not arrived yet, so the answer cannot be assumed empty.
CREATE INDEX IF NOT EXISTS library_recovery_records_unresolved
    ON library_recovery_records(library_root_uuid)
    WHERE unresolved_reason IS NOT NULL;

-- The kind index lets the standalone cleanup and the tombstone lookup reach
-- their rows directly instead of scanning the whole catalog.
CREATE INDEX IF NOT EXISTS library_recovery_records_kind
    ON library_recovery_records(library_root_uuid, row_kind);

-- The record-type index covers the retained record kinds the projection
-- distinguishes. Tombstone rows answer "has this parent been deleted?" for
-- every pending fact, and song rows carry the committed media location a
-- published resource is compared against.
CREATE INDEX IF NOT EXISTS library_recovery_records_record_type
    ON library_recovery_records(library_root_uuid, json_extract(envelope_json, '$.record.type'))
    WHERE row_kind = 'record';
