-- Version 19 existed in development profiles with root-scoped local queues.
-- Version 20 supports both the released v18 layout and those existing v19
-- queues. The migration runner skips only the marked queue-upgrade section
-- when those columns already exist; this complete source is always hashed.
-- Keep each local root's authority history independent. The live tables are
-- the selected protocol context; archived rows are moved atomically with root
-- activation. Local facts and mutation queues remain object/root scoped.
CREATE TABLE IF NOT EXISTS sync_root_contexts (local_root_uuid TEXT PRIMARY KEY NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT);
INSERT OR IGNORE INTO sync_root_contexts SELECT local_root_uuid FROM sync_binding WHERE local_root_uuid IS NOT NULL;
CREATE TABLE IF NOT EXISTS sync_active_context (singleton INTEGER PRIMARY KEY CHECK(singleton=1), local_root_uuid TEXT NOT NULL);
INSERT OR IGNORE INTO sync_active_context SELECT 1, COALESCE((SELECT local_root_uuid FROM sync_binding WHERE singleton=1),(SELECT uuid FROM library_roots WHERE is_active=1),'');

-- BEGIN ROOT QUEUE UPGRADE
ALTER TABLE sync_outbox ADD COLUMN local_root_uuid TEXT NOT NULL DEFAULT '';
UPDATE sync_outbox SET local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1);

ALTER TABLE sync_mutations RENAME TO sync_mutations_before_root_context;
CREATE TABLE sync_mutations (
 mutation_id TEXT PRIMARY KEY NOT NULL, origin_device_id TEXT NOT NULL,
 origin_seq INTEGER NOT NULL CHECK(origin_seq>0), previous_mutation_id TEXT,
 object_type TEXT NOT NULL, object_uuid TEXT NOT NULL,
 operation TEXT NOT NULL CHECK(operation IN ('upsert','delete')),
 local_revision INTEGER NOT NULL CHECK(local_revision>0), base_object_version INTEGER,
 payload_json TEXT NOT NULL, payload_digest TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('pending','confirmed','rejected')),
 outcome_json TEXT, created_at INTEGER NOT NULL, resource_refs_json TEXT NOT NULL DEFAULT '[]',
 local_root_uuid TEXT NOT NULL DEFAULT '',
 UNIQUE(local_root_uuid,origin_device_id,origin_seq)
);
INSERT INTO sync_mutations SELECT *, (SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) FROM sync_mutations_before_root_context;
-- Older versions allocated one origin chain across every local root. Keep
-- its IDs and predecessors intact. For a host, facts belonging to a different
-- root were never eligible for this authority; retain them as rejected history
-- so its cursor can skip them without publishing another library's metadata.
UPDATE sync_mutations SET status='rejected',outcome_json='{"reason":"different_local_library"}'
WHERE status='pending' AND EXISTS(SELECT 1 FROM sync_binding WHERE role='host')
AND (EXISTS(SELECT 1 FROM songs s WHERE s.uuid=sync_mutations.object_uuid AND s.library_root_uuid<>sync_mutations.local_root_uuid)
 OR EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=sync_mutations.object_uuid AND p.library_root_uuid<>sync_mutations.local_root_uuid)
 OR (object_type='playlist-item' AND EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=json_extract(sync_mutations.payload_json,'$.playlist_uuid') AND p.library_root_uuid<>sync_mutations.local_root_uuid)));
DROP TABLE sync_mutations_before_root_context;
CREATE INDEX sync_mutations_pending ON sync_mutations(local_root_uuid,status,origin_device_id,origin_seq);
CREATE INDEX sync_mutations_object ON sync_mutations(object_type,object_uuid,local_revision);

ALTER TABLE sync_local_origin_cursors RENAME TO sync_local_origin_cursors_before_root_context;
CREATE TABLE sync_local_origin_cursors (local_root_uuid TEXT NOT NULL DEFAULT '',origin_device_id TEXT NOT NULL,origin_seq INTEGER NOT NULL CHECK(origin_seq>=0),last_mutation_id TEXT,PRIMARY KEY(local_root_uuid,origin_device_id));
INSERT INTO sync_local_origin_cursors SELECT (SELECT local_root_uuid FROM sync_active_context WHERE singleton=1),origin_device_id,origin_seq,last_mutation_id FROM sync_local_origin_cursors_before_root_context;
DROP TABLE sync_local_origin_cursors_before_root_context;

-- END ROOT QUEUE UPGRADE
CREATE TABLE archived_sync_snapshot_pages AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_snapshot_pages live WHERE 0;
CREATE INDEX archived_sync_snapshot_pages_root ON archived_sync_snapshot_pages(context_root);
CREATE TABLE archived_sync_snapshot_resource_refs AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_snapshot_resource_refs live WHERE 0;
CREATE INDEX archived_sync_snapshot_resource_refs_root ON archived_sync_snapshot_resource_refs(context_root);
CREATE TABLE archived_sync_snapshot_records AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_snapshot_records live WHERE 0;
CREATE INDEX archived_sync_snapshot_records_root ON archived_sync_snapshot_records(context_root);
CREATE TABLE archived_sync_snapshot_sessions AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_snapshot_sessions live WHERE 0;
CREATE INDEX archived_sync_snapshot_sessions_root ON archived_sync_snapshot_sessions(context_root);
CREATE TABLE archived_sync_replica_snapshot_pages AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_replica_snapshot_pages live WHERE 0;
CREATE INDEX archived_sync_replica_snapshot_pages_root ON archived_sync_replica_snapshot_pages(context_root);
CREATE TABLE archived_sync_replica_snapshot_records AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_replica_snapshot_records live WHERE 0;
CREATE INDEX archived_sync_replica_snapshot_records_root ON archived_sync_replica_snapshot_records(context_root);
CREATE TABLE archived_sync_replica_snapshot_sessions AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_replica_snapshot_sessions live WHERE 0;
CREATE INDEX archived_sync_replica_snapshot_sessions_root ON archived_sync_replica_snapshot_sessions(context_root);
CREATE TABLE archived_sync_host_baselines AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_host_baselines live WHERE 0;
CREATE INDEX archived_sync_host_baselines_root ON archived_sync_host_baselines(context_root);
CREATE TABLE archived_sync_transfers AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_transfers live WHERE 0;
CREATE INDEX archived_sync_transfers_root ON archived_sync_transfers(context_root);
CREATE TABLE archived_sync_peer_cursors AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_peer_cursors live WHERE 0;
CREATE INDEX archived_sync_peer_cursors_root ON archived_sync_peer_cursors(context_root);
CREATE TABLE archived_sync_conflicts AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_conflicts live WHERE 0;
CREATE INDEX archived_sync_conflicts_root ON archived_sync_conflicts(context_root);
CREATE TABLE archived_sync_member_aliases AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_member_aliases live WHERE 0;
CREATE INDEX archived_sync_member_aliases_root ON archived_sync_member_aliases(context_root);
CREATE TABLE archived_sync_song_aliases AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_song_aliases live WHERE 0;
CREATE INDEX archived_sync_song_aliases_root ON archived_sync_song_aliases(context_root);
CREATE TABLE archived_sync_object_versions AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_object_versions live WHERE 0;
CREATE INDEX archived_sync_object_versions_root ON archived_sync_object_versions(context_root);
CREATE TABLE archived_sync_commits AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_commits live WHERE 0;
CREATE INDEX archived_sync_commits_root ON archived_sync_commits(context_root);
CREATE TABLE archived_sync_inbox AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_inbox live WHERE 0;
CREATE INDEX archived_sync_inbox_root ON archived_sync_inbox(context_root);
CREATE TABLE archived_sync_origin_cursors AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_origin_cursors live WHERE 0;
CREATE INDEX archived_sync_origin_cursors_root ON archived_sync_origin_cursors(context_root);
CREATE TABLE archived_sync_peers AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_peers live WHERE 0;
CREATE INDEX archived_sync_peers_root ON archived_sync_peers(context_root);
CREATE TABLE archived_sync_binding AS SELECT CAST('' AS TEXT) AS context_root, live.* FROM sync_binding live WHERE 0;
CREATE INDEX archived_sync_binding_root ON archived_sync_binding(context_root);
