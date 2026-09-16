-- A non-null key means the user explicitly chose the playlist artwork. NULL
-- deliberately means "auto": the desktop resolves the newest member's
-- embedded cover at read time.
ALTER TABLE playlists ADD COLUMN cover_asset_key TEXT;
