use super::*;
use crate::application::ports::{HostCommitLedgerPort, HostSnapshotPort, HostSnapshotRecord};
use crate::domain::ids::{PlaylistId, PlaylistItemId};
use crate::domain::sync::{
    AuthorityId, CommitCheckpoint, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
    ObjectKey, ObjectVersion, SyncBinding,
};

fn host() -> (tempfile::TempDir, SqliteDatabase, SyncBinding) {
    let (directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    (directory, database, binding)
}

fn candidate(database: &SqliteDatabase, song: SongId) -> MutationCandidate {
    MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(database.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({"song_uuid":song,"relative_path":"snapshot.flac","title":"published","added_at":1}),
    }
}

fn publish(
    database: &SqliteDatabase,
    binding: SyncBinding,
    candidate: &MutationCandidate,
) -> CommitCheckpoint {
    let checkpoint = database.stage_confirmation(binding, candidate).unwrap();
    assert!(
        checkpoint.checkpoint.is_some(),
        "origin={:?} payload={} outcome={}",
        candidate.origin_device_id,
        candidate.payload,
        checkpoint.outcome_json
    );
    let checkpoint = checkpoint.checkpoint.unwrap();
    database.mark_published(checkpoint).unwrap();
    checkpoint
}

#[test]
fn snapshot_pages_remain_fixed_during_concurrent_later_publication_and_restart() {
    let (directory, database, binding) = host();
    let first_song = SongId::new();
    let first = candidate(&database, first_song);
    let checkpoint = publish(&database, binding, &first);
    let second_id = SongId::new();
    let second = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        object: ObjectKey::Song(second_id),
        payload: serde_json::json!({"song_uuid":second_id,"relative_path":"two.flac","title":"two","added_at":2}),
        ..first.clone()
    };
    let second_checkpoint = publish(&database, binding, &second);
    let session = database.create_host_snapshot(binding, 1, 60_000).unwrap();
    assert_eq!(session.checkpoint, second_checkpoint);
    assert_eq!(session.record_count, 2);
    assert_eq!(session.token.len(), 32);
    let first_page = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    let database = Arc::new(database);
    let writer = {
        let database = Arc::clone(&database);
        std::thread::spawn(move || {
            let later = MutationCandidate {
                mutation_id: MutationId::new(),
                origin_seq: 3,
                previous_mutation_id: Some(second.mutation_id),
                base_object_version: Some(ObjectVersion::from_commit_seq(checkpoint.sequence)),
                payload: serde_json::json!({"song_uuid":first_song,"relative_path":"snapshot.flac","title":"later edit","added_at":1}),
                ..first.clone()
            };
            publish(&database, binding, &later);
        })
    };
    let second_page = database
        .host_snapshot_page(binding, &session.token, 1)
        .unwrap();
    writer.join().unwrap();
    assert_eq!(first_page.next_page_index, Some(1));
    assert_eq!(second_page.next_page_index, None);
    assert!(first_page.records.iter().chain(&second_page.records).all(|record| {
        matches!(record, HostSnapshotRecord::Object {candidate,..} if candidate.payload["title"] != "later edit")
    }));
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .unwrap(),
        first_page
    );
    drop(database);
    let reopened = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert_eq!(
        reopened
            .host_snapshot_page(binding, &session.token, 1)
            .unwrap(),
        second_page
    );
}

#[test]
fn snapshot_excludes_pending_projection_and_preserves_last_live_state_during_undo_window() {
    let (_directory, database, binding) = host();
    let song = SongId::new();
    let first = candidate(&database, song);
    let checkpoint = publish(&database, binding, &first);
    let pending = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        base_object_version: Some(ObjectVersion::from_commit_seq(checkpoint.sequence)),
        payload: serde_json::json!({"song_uuid":song,"relative_path":"snapshot.flac","title":"not published","added_at":1}),
        ..first
    };
    database.stage_confirmation(binding, &pending).unwrap();
    database.writer.run(move |connection| {
        connection.execute("INSERT INTO sync_local_delete_intents(intent_id,final_mutation_id,object_type,object_uuid,created_at,undo_deadline,state) VALUES(?1,?2,'song',?3,1,9223372036854775807,'open')", params![uuid::Uuid::new_v4().to_string(),MutationId::new().as_uuid().to_string(),song.to_string()]).map_err(super::super::support::storage)?;
        connection.execute("UPDATE songs SET availability='pending_delete' WHERE uuid=?1", params![song.to_string()]).map_err(super::super::support::storage)?;
        Ok(())
    }).unwrap();
    let session = database.create_host_snapshot(binding, 16, 60_000).unwrap();
    let page = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    assert_eq!(session.checkpoint, checkpoint);
    assert!(
        matches!(&page.records[0], HostSnapshotRecord::Object {candidate,..} if candidate.payload["title"] == "published")
    );
    assert_eq!(page.records.len(), 1);
}

#[test]
fn snapshot_keeps_final_tombstone_and_aliases_in_one_frozen_record_order() {
    let (_directory, database, binding) = host();
    let song = SongId::new();
    let first = candidate(&database, song);
    let first_checkpoint = publish(&database, binding, &first);
    let deleted = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        base_object_version: Some(ObjectVersion::from_commit_seq(first_checkpoint.sequence)),
        payload: serde_json::json!({"tombstone":true}),
        ..first
    };
    let checkpoint = publish(&database, binding, &deleted);
    let alias = SongId::new();
    database.writer.run(move |connection| {
        connection.execute("INSERT INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) VALUES(?1,?2,?3)", params![alias.to_string(),song.to_string(),i64::try_from(checkpoint.sequence.as_u64()).unwrap()]).map_err(super::super::support::storage)?;
        connection.execute("INSERT INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)", params![PlaylistId::new().to_string(),PlaylistItemId::new().to_string(),PlaylistItemId::new().to_string(),i64::try_from(checkpoint.sequence.as_u64()).unwrap()]).map_err(super::super::support::storage)?;
        Ok(())
    }).unwrap();
    let session = database.create_host_snapshot(binding, 1, 60_000).unwrap();
    assert_eq!(session.record_count, 3);
    let first_page = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    assert!(
        matches!(&first_page.records[0], HostSnapshotRecord::Object { candidate,..} if candidate.payload == serde_json::json!({"tombstone":true}))
    );
    let alias_page = database
        .host_snapshot_page(binding, &session.token, 1)
        .unwrap();
    assert_eq!(
        alias_page.records,
        vec![HostSnapshotRecord::SongAlias {
            alias,
            canonical: song
        }]
    );
    assert_eq!(alias_page.next_page_index, Some(2));
    assert!(matches!(
        &database
            .host_snapshot_page(binding, &session.token, 2)
            .unwrap()
            .records[0],
        HostSnapshotRecord::PlaylistMemberAlias(_)
    ));
}

#[test]
fn snapshot_tokens_bind_to_host_identity_reject_invalid_pages_and_expire_durably() {
    let (_directory, database, binding) = host();
    let session = database.create_host_snapshot(binding, 16, 60_000).unwrap();
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .unwrap()
            .records
            .is_empty()
    );
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 1)
            .is_err()
    );
    assert!(
        database
            .host_snapshot_page(binding, "../invalid", 0)
            .is_err()
    );
    assert!(
        database
            .host_snapshot_page(
                SyncBinding {
                    epoch: Epoch::from_u64(2),
                    ..binding
                },
                &session.token,
                0
            )
            .is_err()
    );
    assert!(
        database
            .host_snapshot_page(binding, &uuid::Uuid::new_v4().simple().to_string(), 0)
            .is_err()
    );
    assert!(database.create_host_snapshot(binding, 0, 60_000).is_err());
    assert!(database.create_host_snapshot(binding, 513, 60_000).is_err());
    assert!(
        database
            .create_host_snapshot(binding, 1, 86_400_001)
            .is_err()
    );
    let token = session.token.clone();
    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "UPDATE sync_snapshot_sessions SET created_at=0,expires_at=1 WHERE token=?1",
                    params![token],
                )
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .is_err()
    );
    database.create_host_snapshot(binding, 1, 60_000).unwrap();
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .is_err()
    );
    let expired_rows: i64 = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_snapshot_sessions WHERE token=?1",
                    params![session.token],
                    |row| row.get(0),
                )
                .map_err(super::super::support::storage)
        })
        .unwrap();
    assert_eq!(expired_rows, 0);
}

#[test]
fn snapshot_rebuilds_merged_device_stats_from_published_events_only() {
    let (_directory, database, binding) = host();
    let song = SongId::new();
    let a = DeviceId::new();
    let b = DeviceId::new();
    database
        .authorize_sync_peer(binding, a, "a".into(), "a-key".into())
        .unwrap();
    database
        .authorize_sync_peer(binding, b, "b".into(), "b-key".into())
        .unwrap();
    let first = MutationCandidate {
        object: ObjectKey::PlayStats(song),
        origin_device_id: a,
        payload: serde_json::json!({
            "song_uuid":song,
            "revision":1,
            "updated_by_device_id":a,
            "hlc":{"wall_secs":1,"counter":0},
            "by_device":{a.as_uuid().to_string():4}
        }),
        ..candidate(&database, song)
    };
    publish(&database, binding, &first);
    let second = MutationCandidate {
        origin_device_id: b,
        mutation_id: MutationId::new(),
        payload: serde_json::json!({
            "song_uuid":song,
            "revision":2,
            "updated_by_device_id":b,
            "hlc":{"wall_secs":2,"counter":0},
            "by_device":{
                a.as_uuid().to_string():4,
                b.as_uuid().to_string():7
            }
        }),
        ..first.clone()
    };
    let checkpoint = publish(&database, binding, &second);
    let pending = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        base_object_version: Some(ObjectVersion::from_commit_seq(checkpoint.sequence)),
        payload: serde_json::json!({
            "song_uuid":song,
            "revision":2,
            "updated_by_device_id":a,
            "hlc":{"wall_secs":3,"counter":0},
            "by_device":{a.as_uuid().to_string():99}
        }),
        ..first
    };
    database.stage_confirmation(binding, &pending).unwrap();
    let session = database.create_host_snapshot(binding, 16, 60_000).unwrap();
    let page = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    let HostSnapshotRecord::Object { candidate, .. } = &page.records[0] else {
        panic!("expected stats")
    };
    assert_eq!(candidate.payload["by_device"][a.as_uuid().to_string()], 4);
    assert_eq!(candidate.payload["by_device"][b.as_uuid().to_string()], 7);
    assert_eq!(session.checkpoint, checkpoint);
}

fn with_resource(
    database: &SqliteDatabase,
    binding: SyncBinding,
    relative_path: &str,
) -> CommitCheckpoint {
    let first = candidate(database, SongId::new());
    let staged = database.stage_confirmation(binding, &first).unwrap();
    let mut checkpoint = staged.checkpoint.unwrap();
    let relative_path = relative_path.to_owned();
    checkpoint.digest = database.writer.run(move |connection| {
        let body:String = connection.query_row("SELECT canonical_json FROM sync_commits WHERE commit_seq=1",[],|row|row.get(0)).map_err(super::super::support::storage)?;
        let mut value:serde_json::Value=serde_json::from_str(&body).unwrap();
        value["resource_refs"]=serde_json::json!([{"kind":"audio","content_hash":"ab".repeat(32),"length":3,"media_type":"audio/flac","relative_path":relative_path}]);
        // Preserve the protocol's exact canonical field order from the fixture.
        let refs = serde_json::to_string(&value["resource_refs"]).unwrap();
        let body=body.replace("\"resource_refs\":[]",&format!("\"resource_refs\":{refs}"));
        let mut hash=blake3::Hasher::new();
        hash.update(b"echo.lan.commit.v1\0");
        hash.update(&1u16.to_be_bytes());
        hash.update(&1u64.to_be_bytes());
        hash.update(&[0;32]);
        hash.update(&u32::try_from(body.len()).unwrap().to_be_bytes());
        hash.update(body.as_bytes());
        let digest=*hash.finalize().as_bytes();
        connection.execute("UPDATE sync_commits SET canonical_json=?1,commit_digest=?2 WHERE commit_seq=1",params![body,digest.as_slice()]).map_err(super::super::support::storage)?;
        Ok(digest)
    }).unwrap();
    database.mark_published(checkpoint).unwrap();
    checkpoint
}

#[test]
fn snapshot_retains_safe_resource_manifests_and_session_hash_references_until_expiry() {
    let (_directory, database, binding) = host();
    with_resource(&database, binding, "media/snapshot.flac");
    let session = database.create_host_snapshot(binding, 16, 60_000).unwrap();
    let page = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    let HostSnapshotRecord::Object { resources, .. } = &page.records[0] else {
        panic!("expected object")
    };
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].digest.to_hex(), "ab".repeat(32));
    assert_eq!(resources[0].relative_path.as_str(), "media/snapshot.flac");
    let token = session.token;
    database
        .writer
        .run(move |connection| {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_snapshot_resource_refs WHERE token=?1",
                    params![token],
                    |row| row.get(0),
                )
                .map_err(super::super::support::storage)?;
            assert_eq!(count, 1);
            connection
                .execute(
                    "UPDATE sync_snapshot_sessions SET created_at=0,expires_at=1 WHERE token=?1",
                    params![token],
                )
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    // The new snapshot owns a new reference; expiry cleanup releases the old.
    let replacement = database.create_host_snapshot(binding, 16, 60_000).unwrap();
    let refs: Vec<String> = database
        .with_reader(|connection| {
            connection
                .prepare("SELECT token FROM sync_snapshot_resource_refs")
                .map_err(super::super::support::storage)?
                .query_map([], |row| row.get(0))
                .map_err(super::super::support::storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(super::super::support::storage)
        })
        .unwrap();
    assert_eq!(refs, vec![replacement.token]);
}

#[test]
fn snapshot_rejects_unsafe_resources_and_corrupted_published_log_without_session() {
    let (_directory, database, binding) = host();
    with_resource(&database, binding, "../outside.flac");
    assert!(database.create_host_snapshot(binding, 16, 60_000).is_err());
    let (_directory, database, binding) = host();
    publish(&database, binding, &candidate(&database, SongId::new()));
    database
        .writer
        .run(|connection| {
            connection
                .execute(
                    "UPDATE sync_commits SET canonical_json='{}' WHERE commit_seq=1",
                    [],
                )
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    assert!(database.create_host_snapshot(binding, 16, 60_000).is_err());
    let sessions: i64 = database
        .with_reader(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM sync_snapshot_sessions", [], |row| {
                    row.get(0)
                })
                .map_err(super::super::support::storage)
        })
        .unwrap();
    assert_eq!(sessions, 0);
}
