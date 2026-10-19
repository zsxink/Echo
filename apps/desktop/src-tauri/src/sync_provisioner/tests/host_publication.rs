use crate::sync_runtime::SyncProvisioner;

use super::*;
use echo_core::application::ports::{LibraryRepository, RemoteResourceJournalPort};
use echo_core::domain::ids::SongId;
use echo_core::domain::sync::ObjectVersion;
use echo_core::infrastructure::filesystem::{Blake3ContentHasher, RootConstrainedFileSystem};
use echo_sync::protocol::ResourceRefDto;

fn audio(path: &str, bytes: &[u8]) -> SharedResourceManifest {
    SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".into(),
        path,
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn commit_peer_song(
    database: &SqliteDatabase,
    binding: SyncBinding,
    peer: DeviceId,
    song: SongId,
    sequence: u64,
    previous: Option<MutationId>,
    base: Option<ObjectVersion>,
    manifest: &SharedResourceManifest,
) -> (CommitCheckpoint, MutationId) {
    let candidate = MutationCandidate {
        resource_refs: vec![manifest.clone()],
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: sequence,
        previous_mutation_id: previous,
        object: ObjectKey::Song(song),
        base_object_version: base,
        payload: serde_json::json!({
            "song_uuid": song,
            "relative_path": manifest.relative_path.as_str(),
            "title": "Peer song",
            "blake3_hash": manifest.digest.to_hex(),
            "file_size": manifest.length,
            "format": "flac",
            "added_at": 1
        }),
    };
    let checkpoint = database
        .stage_confirmation(binding, &candidate)
        .unwrap()
        .checkpoint
        .expect("peer song is accepted");
    database.mark_published(checkpoint).unwrap();
    (checkpoint, candidate.mutation_id)
}

#[test]
#[allow(clippy::too_many_lines)]
fn unavailable_earlier_song_does_not_block_host_publication_of_later_tag_edit() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = echo_desktop::runtime::app::assemble(
        &directory.path().join("host.sqlite"),
        &directory.path().join("covers"),
    )
    .unwrap();
    let database = runtime.database.clone();
    let root_path = directory.path().join("library");
    std::fs::create_dir_all(root_path.join("media")).unwrap();
    prepared_empty_library(&root_path);
    let root = derive_root_id(&root_path.canonicalize().unwrap());
    runtime.registry.register(root, &root_path);
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, root_path.clone(), true, true),
    )
    .unwrap();
    let binding = head(1, 1).binding;
    database.bind_sync_library(binding, true, root).unwrap();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(binding, peer, "Peer".into(), "test-only".into())
        .unwrap();
    let publication = RemoteResourcePublishDeps {
        fs: Arc::new(RootConstrainedFileSystem::new(runtime.registry.clone())),
        hasher: Arc::new(Blake3ContentHasher::new(runtime.registry.clone())),
        journal: database.clone(),
    };
    let provisioner = NativeSyncProvisioner::new(
        database.clone(),
        directory.path().canonicalize().unwrap().join("resources"),
        publication,
        runtime.deps,
        ScanSupervisor::new(),
    );
    let store = provisioner.local_resource_store(binding).unwrap();
    let song = SongId::from_uuid(uuid::Uuid::from_u128(2));
    let blocked = SongId::from_uuid(uuid::Uuid::from_u128(1));
    let path = "media/薛之谦/薛之谦 - 骆驼.flac";
    let original = b"original audio and embedded metadata";
    let edited = b"audio with new embedded cover and lyrics";
    let initial = audio(path, original);
    let (first_checkpoint, first_mutation) =
        commit_peer_song(&database, binding, peer, song, 1, None, None, &initial);
    store
        .import_reader(
            peer,
            &PrepareResource {
                binding,
                resource: ResourceRefDto::from(&initial),
            },
            &mut std::io::Cursor::new(original),
        )
        .unwrap();
    provisioner
        .publish_cached_peer_resources(root, binding, &store)
        .unwrap();
    assert_eq!(std::fs::read(root_path.join(path)).unwrap(), original);
    assert_eq!(
        provisioner
            .sync_progress(SyncRole::Host)
            .unwrap()
            .media_ready,
        Some(true)
    );
    provisioner
        .pending_catalog_changed
        .store(false, Ordering::Release);

    // An older peer song has a declaration but no surviving uploaded bytes.
    // Its deterministic UUID places it ahead of the edited song in the queue.
    let missing = audio("media/missing.flac", b"unavailable audio");
    let (_, second_mutation) = commit_peer_song(
        &database,
        binding,
        peer,
        blocked,
        2,
        Some(first_mutation),
        None,
        &missing,
    );
    let replacement = audio(path, edited);
    commit_peer_song(
        &database,
        binding,
        peer,
        song,
        3,
        Some(second_mutation),
        Some(ObjectVersion::from_commit_seq(first_checkpoint.sequence)),
        &replacement,
    );
    store
        .import_reader(
            peer,
            &PrepareResource {
                binding,
                resource: ResourceRefDto::from(&replacement),
            },
            &mut std::io::Cursor::new(edited),
        )
        .unwrap();
    let sets = database.pending_peer_resource_sets(root).unwrap();
    assert_eq!(sets[0].0, ObjectKey::Song(blocked));
    assert_eq!(sets[1].0, ObjectKey::Song(song));

    provisioner
        .publish_cached_peer_resources(root, binding, &store)
        .unwrap();
    let published = database
        .local_remote_resource_path(root, ObjectKey::Song(song), &replacement)
        .unwrap()
        .expect("later song is published despite the earlier missing resource");
    assert_eq!(
        std::fs::read(root_path.join(published.normalized())).unwrap(),
        edited
    );
    let row = SongRepository::by_id(database.as_ref(), song)
        .unwrap()
        .unwrap();
    assert_eq!(row.path(), &published);
    assert_eq!(
        row.blake3_hash(),
        Some(replacement.digest.to_hex().as_str())
    );
    assert!(provisioner.pending_catalog_changed.load(Ordering::Acquire));
    assert_eq!(database.pending_peer_resource_sets(root).unwrap().len(), 1);
    assert_eq!(
        provisioner
            .sync_progress(SyncRole::Host)
            .unwrap()
            .media_ready,
        Some(false),
        "the host must report the remaining unpublished resource"
    );
    assert_eq!(
        RemoteResourceJournalPort::incomplete(database.as_ref(), root)
            .unwrap()
            .len(),
        1,
        "the failed object's journal remains available for retry"
    );

    // Subsequent passes neither lose the successful association nor create a
    // spurious catalog change while the old failure remains retryable.
    provisioner
        .pending_catalog_changed
        .store(false, Ordering::Release);
    provisioner
        .publish_cached_peer_resources(root, binding, &store)
        .unwrap();
    assert!(!provisioner.pending_catalog_changed.load(Ordering::Acquire));
    assert!(database
        .remote_resource_media_ready(root, ObjectKey::Song(song))
        .unwrap());

    store
        .import_reader(
            peer,
            &PrepareResource {
                binding,
                resource: ResourceRefDto::from(&missing),
            },
            &mut std::io::Cursor::new(b"unavailable audio"),
        )
        .unwrap();
    provisioner
        .publish_cached_peer_resources(root, binding, &store)
        .unwrap();
    assert!(database
        .pending_peer_resource_sets(root)
        .unwrap()
        .is_empty());
    assert_eq!(
        provisioner
            .sync_progress(SyncRole::Host)
            .unwrap()
            .media_ready,
        Some(true),
        "a later upload resumes the original failed journal"
    );
}
