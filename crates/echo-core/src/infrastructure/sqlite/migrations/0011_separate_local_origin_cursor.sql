-- Local event allocation and host-confirmed origin order are distinct facts.
-- The original foundation advanced sync_origin_cursors when a local outbox row
-- was created, which made the host treat an unconfirmed edit as already
-- accepted and reject that same event when the host later confirmed it.
CREATE TABLE sync_local_origin_cursors (
    origin_device_id TEXT PRIMARY KEY NOT NULL,
    origin_seq INTEGER NOT NULL CHECK (origin_seq >= 0),
    last_mutation_id TEXT NOT NULL
);

INSERT INTO sync_local_origin_cursors(origin_device_id, origin_seq, last_mutation_id)
SELECT m.origin_device_id, MAX(m.origin_seq),
       (SELECT m2.mutation_id FROM sync_mutations m2
        WHERE m2.origin_device_id = m.origin_device_id
        ORDER BY m2.origin_seq DESC LIMIT 1)
FROM sync_mutations m
WHERE m.origin_device_id = (SELECT value FROM device_state WHERE key = 'device_id')
GROUP BY m.origin_device_id;

-- Rebuild the local device's host cursor from accepted commits only. Other
-- devices' rows in sync_origin_cursors were already host-confirmed cursors.
DELETE FROM sync_origin_cursors
WHERE origin_device_id = (SELECT value FROM device_state WHERE key = 'device_id');

INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id)
SELECT c.origin_device_id,
       CAST(json_extract(c.canonical_json, '$.mutation.origin_seq') AS INTEGER),
       c.mutation_id
FROM sync_commits c
WHERE c.origin_device_id = (SELECT value FROM device_state WHERE key = 'device_id')
ORDER BY c.commit_seq DESC
LIMIT 1;
