-- A sync binding names a remote logical library, while each installation
-- maps it to its own local root. Persist that mapping; never infer it from the
-- currently active root after a user switches libraries.
ALTER TABLE sync_binding
    ADD COLUMN local_root_uuid TEXT REFERENCES library_roots(uuid) ON DELETE RESTRICT;
