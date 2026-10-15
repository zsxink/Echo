-- Freeze declarations with the event, independent of later object edits.
ALTER TABLE sync_mutations ADD COLUMN resource_refs_json TEXT NOT NULL DEFAULT '[]';
