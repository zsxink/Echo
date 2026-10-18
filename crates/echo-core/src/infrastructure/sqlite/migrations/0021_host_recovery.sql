-- Recovery archives outlive every root context and are never automatically GC'd.
-- Original typed SQL values, mutation IDs and immutable resource declarations
-- stay available after rotating the live host generation.
CREATE TABLE sync_recovery_archives (
 recovery_id TEXT PRIMARY KEY NOT NULL,
 local_root_uuid TEXT NOT NULL REFERENCES library_roots(uuid) ON DELETE RESTRICT,
 previous_binding_json TEXT NOT NULL,
 replacement_binding_json TEXT NOT NULL,
 pending_count INTEGER NOT NULL,
 conflict_count INTEGER NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX sync_recovery_archives_root ON sync_recovery_archives(local_root_uuid,created_at);
CREATE TABLE sync_epoch_tombstones (
 local_root_uuid TEXT NOT NULL,
 object_type TEXT NOT NULL,
 object_uuid TEXT NOT NULL,
 PRIMARY KEY(local_root_uuid,object_type,object_uuid)
);
CREATE TABLE recovery_sync_snapshot_pages AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_snapshot_pages live WHERE 0;
CREATE INDEX recovery_sync_snapshot_pages_id ON recovery_sync_snapshot_pages(recovery_id);
CREATE TABLE recovery_sync_snapshot_resource_refs AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_snapshot_resource_refs live WHERE 0;
CREATE INDEX recovery_sync_snapshot_resource_refs_id ON recovery_sync_snapshot_resource_refs(recovery_id);
CREATE TABLE recovery_sync_snapshot_records AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_snapshot_records live WHERE 0;
CREATE INDEX recovery_sync_snapshot_records_id ON recovery_sync_snapshot_records(recovery_id);
CREATE TABLE recovery_sync_snapshot_sessions AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_snapshot_sessions live WHERE 0;
CREATE INDEX recovery_sync_snapshot_sessions_id ON recovery_sync_snapshot_sessions(recovery_id);
CREATE TABLE recovery_sync_replica_snapshot_pages AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_replica_snapshot_pages live WHERE 0;
CREATE INDEX recovery_sync_replica_snapshot_pages_id ON recovery_sync_replica_snapshot_pages(recovery_id);
CREATE TABLE recovery_sync_replica_snapshot_records AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_replica_snapshot_records live WHERE 0;
CREATE INDEX recovery_sync_replica_snapshot_records_id ON recovery_sync_replica_snapshot_records(recovery_id);
CREATE TABLE recovery_sync_replica_snapshot_sessions AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_replica_snapshot_sessions live WHERE 0;
CREATE INDEX recovery_sync_replica_snapshot_sessions_id ON recovery_sync_replica_snapshot_sessions(recovery_id);
CREATE TABLE recovery_sync_host_baselines AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_host_baselines live WHERE 0;
CREATE INDEX recovery_sync_host_baselines_id ON recovery_sync_host_baselines(recovery_id);
CREATE TABLE recovery_sync_transfers AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_transfers live WHERE 0;
CREATE INDEX recovery_sync_transfers_id ON recovery_sync_transfers(recovery_id);
CREATE TABLE recovery_sync_peer_cursors AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_peer_cursors live WHERE 0;
CREATE INDEX recovery_sync_peer_cursors_id ON recovery_sync_peer_cursors(recovery_id);
CREATE TABLE recovery_sync_conflicts AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_conflicts live WHERE 0;
CREATE INDEX recovery_sync_conflicts_id ON recovery_sync_conflicts(recovery_id);
CREATE TABLE recovery_sync_member_aliases AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_member_aliases live WHERE 0;
CREATE INDEX recovery_sync_member_aliases_id ON recovery_sync_member_aliases(recovery_id);
CREATE TABLE recovery_sync_song_aliases AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_song_aliases live WHERE 0;
CREATE INDEX recovery_sync_song_aliases_id ON recovery_sync_song_aliases(recovery_id);
CREATE TABLE recovery_sync_object_versions AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_object_versions live WHERE 0;
CREATE INDEX recovery_sync_object_versions_id ON recovery_sync_object_versions(recovery_id);
CREATE TABLE recovery_sync_commits AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_commits live WHERE 0;
CREATE INDEX recovery_sync_commits_id ON recovery_sync_commits(recovery_id);
CREATE TABLE recovery_sync_inbox AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_inbox live WHERE 0;
CREATE INDEX recovery_sync_inbox_id ON recovery_sync_inbox(recovery_id);
CREATE TABLE recovery_sync_origin_cursors AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_origin_cursors live WHERE 0;
CREATE INDEX recovery_sync_origin_cursors_id ON recovery_sync_origin_cursors(recovery_id);
CREATE TABLE recovery_sync_peers AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_peers live WHERE 0;
CREATE INDEX recovery_sync_peers_id ON recovery_sync_peers(recovery_id);
CREATE TABLE recovery_sync_binding AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_binding live WHERE 0;
CREATE INDEX recovery_sync_binding_id ON recovery_sync_binding(recovery_id);
CREATE TABLE recovery_sync_mutations AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_mutations live WHERE 0;
CREATE INDEX recovery_sync_mutations_id ON recovery_sync_mutations(recovery_id);
CREATE TABLE recovery_sync_outbox AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_outbox live WHERE 0;
CREATE INDEX recovery_sync_outbox_id ON recovery_sync_outbox(recovery_id);
CREATE TABLE recovery_sync_local_origin_cursors AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_local_origin_cursors live WHERE 0;
CREATE INDEX recovery_sync_local_origin_cursors_id ON recovery_sync_local_origin_cursors(recovery_id);
CREATE TABLE recovery_sync_remote_resource_locations AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_remote_resource_locations live WHERE 0;
CREATE INDEX recovery_sync_remote_resource_locations_id ON recovery_sync_remote_resource_locations(recovery_id);
CREATE TABLE recovery_sync_remote_publication_claims AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_remote_publication_claims live WHERE 0;
CREATE INDEX recovery_sync_remote_publication_claims_id ON recovery_sync_remote_publication_claims(recovery_id);
CREATE TABLE recovery_sync_remote_publications AS SELECT CAST('' AS TEXT) AS recovery_id, live.* FROM sync_remote_publications live WHERE 0;
CREATE INDEX recovery_sync_remote_publications_id ON recovery_sync_remote_publications(recovery_id);
