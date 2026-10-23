use super::*;
use crate::application::ports::{
    HostCommitLedgerPort, LocalSyncQueuePort, RemoteResourceItem, RemoteResourceJournalPort,
    RemoteResourcePublication, RemoteResourceStage, RemoteResourceTarget,
};
use crate::domain::sync::{
    AuthorityId, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
    ResourceDigest, SharedResourceKind, SharedResourceManifest, SyncBinding,
};

#[path = "sync_resource_publication_safety.rs"]
mod safety;

#[path = "sync_resource_publication_durability.rs"]
mod durability;

fn fixture() -> (
    tempfile::TempDir,
    SqliteDatabase,
    LibraryRootId,
    ObjectKey,
    SharedResourceManifest,
) {
    let (directory, database, root) = database();
    let manifest = manifest(b"initial audio", "media/track.flac");
    let mut song = song(root, "media/track.flac", "Track", "Artist");
    song.apply_scan_facts(
        manifest.digest.to_hex(),
        manifest.length,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&database, &song).expect("insert song");
    let object = ObjectKey::Song(song.id());
    database
        .set_local_sync_resources(object, vec![manifest.clone()])
        .expect("declare current resource");
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(1),
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    database
        .ensure_host_sync_baseline(binding, root)
        .expect("publish initial host baseline");
    let mut initial_song = database
        .pending_local_mutations(32)
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.object == object)
        .expect("baseline song mutation");
    initial_song.resource_refs = vec![manifest.clone()];
    let checkpoint = database
        .stage_confirmation(binding, &initial_song)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(checkpoint).unwrap();
    (directory, database, root, object, manifest)
}

fn manifest(bytes: &[u8], path: &str) -> SharedResourceManifest {
    SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".to_owned(),
        path,
    )
    .expect("valid resource manifest")
}

fn publication(
    root: LibraryRootId,
    object: ObjectKey,
    manifest: SharedResourceManifest,
    expected_old_digest: Option<ResourceDigest>,
) -> RemoteResourcePublication {
    let operation = OperationId::new();
    RemoteResourcePublication {
        root,
        operation,
        object,
        items: vec![RemoteResourceItem {
            target: RemoteResourceTarget {
                manifest,
                expected_old_digest,
            },
            stage: RemoteResourceStage::Planned,
            staging_path: format!("echo/tmp/{operation}/audio.part"),
            local_path: "media/track.flac".to_owned(),
            old_path: None,
        }],
        completed: false,
    }
}

#[test]
fn remote_resource_publication_is_durable_and_marks_media_ready_only_after_finish() {
    let (_directory, database, root, object, manifest) = fixture();
    assert!(!database
        .remote_resource_media_ready(root, object)
        .expect("initially missing"));
    let pending = database
        .pending_remote_resource_sets(root)
        .expect("pending manifest");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, object);
    assert_eq!(pending[0].1[0].expected_old_digest, None);

    let mut publication = publication(root, object, manifest.clone(), None);
    database.prepare(&publication).expect("reserve publication");
    assert_eq!(database.incomplete(root).unwrap(), [publication.operation]);
    assert_eq!(
        RemoteResourceJournalPort::load(&database, root, publication.operation).unwrap(),
        publication
    );
    assert!(database
        .local_remote_resource_path(root, object, &manifest)
        .unwrap()
        .is_none());

    publication.items[0].stage = RemoteResourceStage::Published;
    database
        .save(&publication)
        .expect("persist publication stage");
    database
        .finish(&publication)
        .expect("associate published media");
    assert!(database
        .remote_resource_media_ready(root, object)
        .expect("media ready"));
    assert!(database
        .pending_remote_resource_sets(root)
        .unwrap()
        .is_empty());
    assert_eq!(
        database
            .local_remote_resource_path(root, object, &manifest)
            .unwrap(),
        Some(RelativeMediaPath::new("media/track.flac").unwrap())
    );
    assert!(database.incomplete(root).unwrap().is_empty());
    let completed =
        RemoteResourceJournalPort::load(&database, root, publication.operation).unwrap();
    assert!(completed.completed);
    assert_eq!(completed.items[0].stage, RemoteResourceStage::Completed);
    database
        .finish(&completed)
        .expect("finish retry is idempotent");
}

#[test]
fn host_only_schedules_peer_authored_resources_for_publication() {
    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(1),
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    let host_device = DeviceId::from_uuid(database.current_device_id().as_uuid());
    let peer_device = DeviceId::new();
    database
        .authorize_sync_peer(
            binding,
            peer_device,
            "Peer device".to_owned(),
            "secure-store://peer".to_owned(),
        )
        .expect("authorize peer");
    let host_song = song(root, "media/host.flac", "Host", "Artist");
    SongRepository::upsert(&database, &host_song).expect("local song");
    let host_resource = manifest(b"host bytes", "media/host.flac");
    let host_candidate = MutationCandidate {
        resource_refs: vec![host_resource],
        mutation_id: MutationId::new(),
        origin_device_id: host_device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(host_song.id()),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": host_song.id(),
            "relative_path": "media/host.flac",
            "title": "Host",
            "artist": "Artist",
            "blake3_hash": manifest(b"host bytes", "media/host.flac").digest.to_hex(),
            "added_at": 1
        }),
    };
    let host_commit = database
        .stage_confirmation(binding, &host_candidate)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(host_commit).unwrap();

    let peer_song = SongId::new();
    let peer_resource = manifest(b"peer bytes", "media/peer.flac");
    let peer_candidate = MutationCandidate {
        resource_refs: vec![peer_resource],
        mutation_id: MutationId::new(),
        origin_device_id: peer_device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(peer_song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": peer_song,
            "relative_path": "media/peer.flac",
            "title": "Peer",
            "artist": "Artist",
            "blake3_hash": manifest(b"peer bytes", "media/peer.flac").digest.to_hex(),
            "added_at": 2
        }),
    };
    let peer_commit = database
        .stage_confirmation(binding, &peer_candidate)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(peer_commit).unwrap();

    let all = database.pending_remote_resource_sets(root).unwrap();
    assert_eq!(all.len(), 2);
    let peer_pending = database.pending_peer_resource_sets(root).unwrap();
    assert_eq!(peer_pending.len(), 1);
    assert_eq!(peer_pending[0].0, ObjectKey::Song(peer_song));
    assert_eq!(
        peer_pending[0].1[0].manifest.relative_path.as_str(),
        "media/peer.flac"
    );
}

#[test]
fn remote_resource_replacement_requires_the_exact_previous_digest() {
    let (_directory, database, root, object, initial) = fixture();
    let mut first = publication(root, object, initial.clone(), None);
    database.prepare(&first).unwrap();
    first.items[0].stage = RemoteResourceStage::Published;
    database.save(&first).unwrap();
    database.finish(&first).unwrap();

    let replacement = manifest(b"replacement audio", "media/track.flac");
    database
        .set_local_sync_resources(object, vec![replacement.clone()])
        .unwrap();
    let pending = database.pending_remote_resource_sets(root).unwrap();
    assert_eq!(pending[0].1[0].expected_old_digest, Some(initial.digest));
    assert!(!database.remote_resource_media_ready(root, object).unwrap());

    let wrong_old = ResourceDigest::from_bytes([0x55; 32]);
    let rejected = publication(root, object, replacement.clone(), Some(wrong_old));
    assert!(database.prepare(&rejected).is_err());
    assert!(database.incomplete(root).unwrap().is_empty());

    let mut valid = publication(root, object, replacement, Some(initial.digest));
    database.prepare(&valid).unwrap();
    valid = RemoteResourceJournalPort::load(&database, root, valid.operation).unwrap();
    assert_eq!(valid.items[0].old_path.as_deref(), Some("media/track.flac"));
    valid.items[0].stage = RemoteResourceStage::Published;
    database.save(&valid).unwrap();
    database.finish(&valid).unwrap();
    assert!(database.remote_resource_media_ready(root, object).unwrap());
}

#[test]
fn remote_resource_journal_rejects_stale_root_reused_identity_and_incomplete_finish() {
    let (_directory, database, root, object, initial) = fixture();
    let incomplete = publication(root, object, initial, None);
    database.prepare(&incomplete).unwrap();
    assert!(database.finish(&incomplete).is_err());

    let changed = manifest(b"different bytes", "media/other.flac");
    let reused = RemoteResourcePublication {
        items: vec![RemoteResourceItem {
            target: RemoteResourceTarget {
                manifest: changed,
                expected_old_digest: None,
            },
            ..incomplete.items[0].clone()
        }],
        ..incomplete.clone()
    };
    assert!(database.prepare(&reused).is_err());

    let changed_declaration = manifest(b"new current declaration", "media/track.flac");
    database
        .set_local_sync_resources(object, vec![changed_declaration.clone()])
        .unwrap();
    assert!(RemoteResourceJournalPort::load(&database, root, incomplete.operation).is_ok());
    assert!(database.finish(&incomplete).is_err());

    let other_root = LibraryRootId::new();
    let stale_root = publication(other_root, object, changed_declaration, None);
    assert!(database.prepare(&stale_root).is_err());
    assert_eq!(database.incomplete(root).unwrap(), [incomplete.operation]);
}

#[test]
fn maintenance_repairs_previously_acked_tags_without_new_commits_or_file_changes() {
    for indexed in [false, true] {
        let (_directory, database, root, object, manifest) = fixture();
        let ObjectKey::Song(id) = object else {
            unreachable!()
        };
        let mut publication = publication(root, object, manifest, None);
        database.prepare(&publication).unwrap();
        publication.items[0].stage = RemoteResourceStage::Published;
        database.save(&publication).unwrap();
        database.finish(&publication).unwrap();
        database.writer.run(move |connection| {
            connection.execute("UPDATE sync_mutations SET status='confirmed'", []).map_err(storage)?;
            connection.execute(
                "UPDATE sync_object_versions SET payload_json=json_set(payload_json,'$.album','New album','$.duration_ms',json('18446744073709551615'),'$.added_at',json('18446744073709551615')) WHERE object_type='song' AND object_uuid=?1",
                params![id.to_string()],
            ).map_err(storage)?;
            connection.execute(
                "UPDATE songs SET album='Old album',file_mtime_ns=?2 WHERE uuid=?1",
                params![id.to_string(), indexed.then_some(123_i64)],
            ).map_err(storage)?;
            Ok(())
        }).unwrap();
        let before = SongRepository::by_id(&database, id).unwrap().unwrap();
        let queue = database.pending_local_mutations(32).unwrap();
        assert_eq!(
            database
                .repair_published_song_projections(LibraryRootId::new())
                .unwrap(),
            0
        );
        assert_eq!(database.repair_published_song_projections(root).unwrap(), 1);
        let after = SongRepository::by_id(&database, id).unwrap().unwrap();
        assert_eq!(after.album(), Some("New album"));
        assert_eq!(after.path(), before.path());
        assert_eq!(after.blake3_hash(), before.blake3_hash());
        assert_eq!(after.file_mtime_ns(), before.file_mtime_ns());
        assert_eq!(after.availability(), before.availability());
        assert_eq!(
            database.pending_local_mutations(32).unwrap().len(),
            queue.len()
        );
        assert_eq!(database.repair_published_song_projections(root).unwrap(), 0);
    }
}

fn completed_missing_song() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId, SongId) {
    let (directory, database, root, object, manifest) = fixture();
    let ObjectKey::Song(id) = object else {
        unreachable!()
    };
    let mut publication = publication(root, object, manifest, None);
    database.prepare(&publication).unwrap();
    publication.items[0].stage = RemoteResourceStage::Published;
    database.save(&publication).unwrap();
    database.finish(&publication).unwrap();
    database.writer.run(move |connection| {
        connection.execute("UPDATE sync_mutations SET status='confirmed'", []).map_err(storage)?;
        connection.execute("UPDATE songs SET availability='missing',file_mtime_ns=123,album='Old album' WHERE uuid=?1", params![id.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    (directory, database, root, id)
}

#[test]
fn completed_missing_song_is_queued_and_restored_without_a_shared_mutation() {
    let (_directory, database, root, id) = completed_missing_song();
    assert_eq!(database.pending_remote_song_metadata(root).unwrap(), [id]);
    assert!(database
        .pending_remote_resource_sets(root)
        .unwrap()
        .is_empty());
    let before = SongRepository::by_id(&database, id).unwrap().unwrap();
    assert_eq!(before.file_mtime_ns(), Some(123));
    let queue = database.pending_local_mutations(512).unwrap();
    let revision = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT updated_at FROM library_roots WHERE uuid=?1",
                    params![root.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(storage)
        })
        .unwrap();
    let mut indexed = before.clone();
    indexed.apply_scan_facts(
        before.blake3_hash().unwrap().to_owned(),
        before.file_size().unwrap(),
        456,
        crate::domain::media::AudioFormat::Flac,
    );
    database
        .with_tx(Box::new(move |tx| tx.refresh_song_file_index(&indexed)))
        .unwrap();
    let after = SongRepository::by_id(&database, id).unwrap().unwrap();
    assert_eq!(after.availability(), SongAvailability::Available);
    assert_eq!(after.album(), Some("专辑"));
    assert_eq!(after.path(), before.path());
    assert_eq!(after.blake3_hash(), before.blake3_hash());
    assert_eq!(after.file_mtime_ns(), Some(456));
    assert!(
        database
            .with_reader(|connection| connection
                .query_row(
                    "SELECT updated_at FROM library_roots WHERE uuid=?1",
                    params![root.to_string()],
                    |row| row.get::<_, i64>(0)
                )
                .map_err(storage))
            .unwrap()
            > revision
    );
    assert_eq!(database.pending_local_mutations(512).unwrap(), queue);
    assert!(database
        .pending_remote_song_metadata(root)
        .unwrap()
        .is_empty());
}

#[test]
fn completed_missing_song_recovery_rechecks_current_association_and_delete_guards() {
    for change in [
        "path",
        "manifest",
        "publication",
        "pending_delete",
        "tombstone",
        "alias",
        "pending_edit",
        "delete_intent",
        "binding",
    ] {
        let (_directory, database, root, id) = completed_missing_song();
        let before = SongRepository::by_id(&database, id).unwrap().unwrap();
        database.writer.run(move |connection| {
            let statement = match change {
                "path" => "UPDATE songs SET relative_path='media/changed.flac',normalized_relative_path='media/changed.flac'",
                "manifest" => "UPDATE sync_resource_manifests SET content_hash=zeroblob(32)",
                "publication" => "UPDATE sync_remote_publications SET completed=0",
                "pending_delete" => "UPDATE songs SET availability='pending_delete'",
                "tombstone" => "UPDATE sync_object_versions SET is_tombstone=1",
                "alias" => "INSERT INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) SELECT uuid,'00000000-0000-0000-0000-000000000001',1 FROM songs",
                "pending_edit" => "UPDATE sync_mutations SET status='pending'",
                "delete_intent" => "INSERT INTO sync_local_delete_intents(intent_id,final_mutation_id,object_type,object_uuid,created_at,undo_deadline,state) SELECT 'intent','mutation','song',uuid,1,2,'open' FROM songs",
                "binding" => "UPDATE sync_binding SET local_root_uuid=NULL",
                _ => unreachable!(),
            };
            connection.execute(statement, []).map_err(storage)?;
            Ok(())
        }).unwrap();
        assert!(
            database
                .pending_remote_song_metadata(root)
                .unwrap()
                .is_empty(),
            "{change}"
        );
        assert!(
            database
                .with_tx(Box::new(move |tx| tx.refresh_song_file_index(&before)))
                .is_err(),
            "{change}"
        );
        assert_ne!(
            SongRepository::by_id(&database, id)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Available,
            "{change}"
        );
    }
}
