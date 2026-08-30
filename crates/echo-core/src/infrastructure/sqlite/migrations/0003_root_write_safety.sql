-- A destructive filesystem result can be indeterminate. Keep that safety
-- isolation independently from permissions so root re-selection cannot reopen
-- imports or deletes without explicit recovery.
ALTER TABLE library_roots
    ADD COLUMN write_safety_locked INTEGER NOT NULL DEFAULT 0
    CHECK (write_safety_locked IN (0, 1));
