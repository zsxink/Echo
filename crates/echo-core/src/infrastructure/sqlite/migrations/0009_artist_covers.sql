-- Artist identities are normalized by the Core catalog query and scoped to a
-- library root. NULL never needs a row: deleting the row restores automatic
-- artwork from the latest-added album.
CREATE TABLE artist_covers (
    library_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid),
    artist_key TEXT NOT NULL,
    cover_asset_key TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (library_root_uuid, artist_key)
);
