-- Persist per-song audio stream parameters read by the metadata probe at scan
-- time (task 6.4). The read-only song-detail DTO reads these from the committed
-- row and never re-probes the file, so a detail view is stable and cheap.
ALTER TABLE songs
    ADD COLUMN bitrate_bps INTEGER;
ALTER TABLE songs
    ADD COLUMN sample_rate_hz INTEGER;
ALTER TABLE songs
    ADD COLUMN channels INTEGER;
ALTER TABLE songs
    ADD COLUMN bits_per_sample INTEGER;