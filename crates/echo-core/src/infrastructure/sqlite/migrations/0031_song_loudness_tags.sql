-- Per-song embedded loudness tags read by the metadata reader at scan time.
-- These are fixed-point integers (milli-dB / parts-per-million of full scale)
-- so the domain entity keeps its Eq identity; see domain/media.rs.
-- Local scan-derived facts only: they never enter the sync snapshot payload,
-- the portable records, the outbox JSON, or the media file itself.
ALTER TABLE songs ADD COLUMN loudness_source TEXT;
ALTER TABLE songs ADD COLUMN loudness_track_gain_mdb INTEGER;
ALTER TABLE songs ADD COLUMN loudness_album_gain_mdb INTEGER;
ALTER TABLE songs ADD COLUMN loudness_track_peak_ppm INTEGER;
ALTER TABLE songs ADD COLUMN loudness_album_peak_ppm INTEGER;
