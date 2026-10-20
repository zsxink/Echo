-- A song's probed audio properties are not competing user edits. Older
-- versions recorded false conflicts when one device had these facts and the
-- other submitted nulls for the same media hash. Retain the audit payloads,
-- but close those conflicts in live, archived, and recovery ledgers.
UPDATE sync_conflicts
SET resolved_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000
WHERE resolved_at IS NULL
  AND object_type = 'song'
  AND json_valid(replaced_payload_json)
  AND json_valid(accepted_payload_json)
  AND json_remove(replaced_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample')
    = json_remove(accepted_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample');

UPDATE archived_sync_conflicts
SET resolved_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000
WHERE resolved_at IS NULL
  AND object_type = 'song'
  AND json_valid(replaced_payload_json)
  AND json_valid(accepted_payload_json)
  AND json_remove(replaced_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample')
    = json_remove(accepted_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample');

UPDATE recovery_sync_conflicts
SET resolved_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000
WHERE resolved_at IS NULL
  AND object_type = 'song'
  AND json_valid(replaced_payload_json)
  AND json_valid(accepted_payload_json)
  AND json_remove(replaced_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample')
    = json_remove(accepted_payload_json,
      '$.bitrate_bps', '$.sample_rate_hz', '$.channels', '$.bits_per_sample');
