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
