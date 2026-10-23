use super::*;
use echo_core::application::ports::{
    HostCommitLedgerPort, HostSnapshotPort, RemoteApplyPort, RemoteCommit,
    ReplicaSnapshotApplyPort, ReplicaSnapshotPage, SongRepository,
};
use echo_core::application::root_switch::derive_root_id;
use echo_core::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
    MutationId, ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest, SyncBinding,
};
use echo_core::infrastructure::filesystem::staging::StagingMarker;
use echo_sync::resource_transfer::{DownloadChunk, TransferError, TransferProgress};

fn prepared_empty_library(root: &Path) {
    std::fs::create_dir_all(root.join("echo/tmp")).unwrap();
    std::fs::write(
        root.join("echo/manifest.json"),
        serde_json::to_vec(&echo_core::domain::library::LibraryManifest::new(
            echo_core::domain::library::LibraryId::from_uuid(uuid::Uuid::new_v4()),
            env!("CARGO_PKG_VERSION"),
        ))
        .unwrap(),
    )
    .unwrap();
    let canonical = root.canonicalize().unwrap();
    std::fs::write(
        root.join("echo/tmp").join(EMPTY_LIBRARY_STAGING_MARKER),
        StagingMarker::new(derive_root_id(&canonical)).body(),
    )
    .unwrap();
}

#[test]
fn selected_empty_library_scaffold_is_enrollable_but_user_content_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    std::fs::create_dir(&root).unwrap();
    prepared_empty_library(&root);
    std::fs::create_dir(root.join("media")).unwrap();
    assert!(is_empty_enrollment_root(&root));

    std::fs::write(root.join("media/track.flac"), b"audio").unwrap();
    assert!(!is_empty_enrollment_root(&root));
}

#[test]
fn enrollment_scaffold_rejects_unrelated_files_and_forged_staging_markers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    std::fs::create_dir(&root).unwrap();
    prepared_empty_library(&root);
    assert!(is_empty_enrollment_root(&root));

    std::fs::write(root.join("notes.txt"), b"keep me").unwrap();
    assert!(!is_empty_enrollment_root(&root));
    std::fs::remove_file(root.join("notes.txt")).unwrap();
    std::fs::write(
        root.join("echo/tmp").join(EMPTY_LIBRARY_STAGING_MARKER),
        b"not Echo's marker",
    )
    .unwrap();
    assert!(!is_empty_enrollment_root(&root));
}

#[test]
fn first_pairing_prepares_only_an_empty_selected_directory_for_enrollment() {
    let directory = tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty-library");
    std::fs::create_dir(&empty).unwrap();
    let root = derive_root_id(&empty.canonicalize().unwrap());
    prepare_empty_enrollment_root(&empty, root).unwrap();
    assert!(is_empty_enrollment_root(&empty));
    let manifest: echo_core::domain::library::LibraryManifest =
        serde_json::from_slice(&std::fs::read(empty.join("echo/manifest.json")).unwrap()).unwrap();
    assert!(manifest.is_compatible());
    let marker =
        std::fs::read_to_string(empty.join("echo/tmp").join(EMPTY_LIBRARY_STAGING_MARKER)).unwrap();
    assert_eq!(StagingMarker::parse(&marker).unwrap().root, root);

    let occupied = directory.path().join("occupied-library");
    std::fs::create_dir(&occupied).unwrap();
    std::fs::write(occupied.join("keep.txt"), b"user data").unwrap();
    assert_eq!(
        prepare_empty_enrollment_root(&occupied, LibraryRootId::new()),
        Err(SyncRuntimeError::LibraryUnavailable)
    );
    assert_eq!(
        std::fs::read(occupied.join("keep.txt")).unwrap(),
        b"user data"
    );
    assert!(!occupied.join("echo").exists());
}

#[test]
fn enrollment_initializes_a_valid_manifest_without_replacing_existing_content() {
    let directory = tempfile::tempdir().unwrap();
    let root_path = directory.path().join("library");
    std::fs::create_dir_all(root_path.join("media")).unwrap();
    let media = root_path.join("media/keep.flac");
    std::fs::write(&media, b"existing audio").unwrap();
    let root = derive_root_id(&root_path.canonicalize().unwrap());
    prepare_enrollment_root(&root_path, root).unwrap();
    let path = root_path.join("echo/manifest.json");
    let original = std::fs::read(&path).unwrap();
    let manifest: echo_core::domain::library::LibraryManifest =
        serde_json::from_slice(&original).unwrap();
    assert!(manifest.is_compatible());
    prepare_enrollment_root(&root_path, root).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(std::fs::read(media).unwrap(), b"existing audio");
}

fn head(sequence: u64, digest: u8) -> CommitCheckpoint {
    static BINDING: std::sync::OnceLock<SyncBinding> = std::sync::OnceLock::new();
    CommitCheckpoint {
        binding: *BINDING.get_or_init(|| SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        }),
        sequence: CommitSeq::from_u64(sequence),
        digest: [digest; 32],
    }
}

#[test]
fn startup_allows_checkpoint_written_before_publication_marking() {
    assert_eq!(
        validate_startup_head(head(2, 2), head(1, 1), &[head(2, 2)]),
        Ok(())
    );
    assert_eq!(
        validate_startup_head(head(1, 1), head(1, 1), &[head(2, 2)]),
        Ok(())
    );
}

#[test]
fn startup_refuses_restored_or_forked_history_before_repair() {
    assert!(validate_startup_head(head(2, 2), head(1, 1), &[]).is_err());
    assert!(validate_startup_head(head(2, 2), head(1, 1), &[head(2, 9)]).is_err());
    assert!(validate_startup_head(head(1, 9), head(1, 1), &[]).is_err());
    assert!(validate_startup_head(head(1, 1), head(2, 2), &[head(3, 3)]).is_err());
    let wrong_binding = CommitCheckpoint {
        binding: SyncBinding {
            authority_id: AuthorityId::new(),
            ..head(2, 2).binding
        },
        ..head(2, 2)
    };
    assert!(validate_startup_head(head(1, 1), head(1, 1), &[wrong_binding]).is_err());
}

#[test]
fn audio_seed_skips_missing_but_refuses_replaced_frozen_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("media")).unwrap();
    let library = LibraryRoot::new(LibraryRootId::new(), root.clone(), true, true);
    let store =
        FileResourceStore::new(directory.path().canonicalize().unwrap().join("resources")).unwrap();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([1; 32]),
        7,
        "audio/flac".to_owned(),
        "media/track.flac",
    )
    .unwrap();
    let binding = head(1, 1).binding;
    let device = DeviceId::new();
    assert_eq!(
        seed_audio_resources(
            &store,
            &library,
            binding,
            device,
            std::slice::from_ref(&resource)
        ),
        Ok(())
    );
    std::fs::write(root.join("media/track.flac"), b"changed").unwrap();
    assert_eq!(
        seed_audio_resources(&store, &library, binding, device, &[resource]),
        Err(SyncRuntimeError::LibraryUnavailable)
    );
}

#[test]
fn source_authorization_refuses_traversal_and_symlink_components() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(directory.path().join("outside.flac"), b"outside").unwrap();
    assert_eq!(
        authorized_source(&root, "../outside.flac"),
        Err(SyncRuntimeError::LibraryUnavailable)
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(directory.path(), root.join("linked")).unwrap();
        assert_eq!(
            authorized_source(&root, "linked/outside.flac"),
            Err(SyncRuntimeError::LibraryUnavailable)
        );
    }
}

#[test]
fn cancel_of_unknown_pairing_generation_preserves_previous_authorization() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    echo_core::application::ports::LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().to_path_buf(), true, true),
    )
    .unwrap();
    let binding = head(1, 1).binding;
    database.bind_sync_library(binding, true, root).unwrap();
    let device_id = DeviceId::new();
    database
        .authorize_sync_peer(
            binding,
            device_id,
            "Existing peer".to_owned(),
            "prior-generation".to_owned(),
        )
        .unwrap();
    let request = PendingPairingRequest {
        binding,
        request_id: "a".repeat(64),
        device_id,
        device_name: "New request".to_owned(),
        expires_in_secs: 30,
    };
    let pairing = DurableHostPairing::new(database.clone());
    assert_eq!(pairing.cancel_approval(&request), Ok(()));
    assert!(database
        .sync_peer_has_credential(binding, device_id, "prior-generation".to_owned())
        .unwrap());
    // The fake prior-generation handle has no OS credential to recover.
    assert!(matches!(
        pairing.approve(&request),
        Err(PairingError::ProvisioningUnavailable)
    ));
    assert!(database
        .sync_peer_has_credential(binding, device_id, "prior-generation".to_owned())
        .unwrap());
}

fn publish_test_song(
    database: &SqliteDatabase,
    binding: SyncBinding,
    song: echo_core::domain::ids::SongId,
    origin_seq: u64,
    previous: Option<MutationId>,
    resources: Vec<SharedResourceManifest>,
) -> (CommitCheckpoint, MutationId) {
    let candidate = MutationCandidate {
        resource_refs: resources,
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(database.current_device_id().as_uuid()),
        origin_seq,
        previous_mutation_id: previous,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": song,
            "relative_path": format!("media/{song}.flac"),
            "title": format!("Song {origin_seq}"),
            "added_at": origin_seq
        }),
    };
    let staged = database.stage_confirmation(binding, &candidate).unwrap();
    let checkpoint = staged.checkpoint.expect("test mutation accepted");
    database.mark_published(checkpoint).unwrap();
    (checkpoint, candidate.mutation_id)
}

#[test]
#[allow(clippy::too_many_lines)]
fn host_publishes_peer_uploaded_audio_into_its_library_before_reporting_ready() {
    use echo_core::application::ports::{LibraryRepository, RemoteResourceJournalPort};
    use echo_core::infrastructure::filesystem::{
        Blake3ContentHasher, RootConstrainedFileSystem, RootRegistry,
    };
    use echo_sync::protocol::ResourceRefDto;
    use echo_sync::resource_transfer::PrepareResource;

    let directory = tempfile::tempdir().unwrap();
    let root_path = directory.path().join("host-library");
    std::fs::create_dir_all(root_path.join("echo/tmp")).unwrap();
    std::fs::create_dir_all(root_path.join("media")).unwrap();
    let root = derive_root_id(&root_path.canonicalize().unwrap());
    std::fs::write(
        root_path
            .join("echo/tmp")
            .join(EMPTY_LIBRARY_STAGING_MARKER),
        StagingMarker::new(root).body(),
    )
    .unwrap();
    let runtime = echo_desktop::runtime::app::assemble(
        &directory.path().join("host.sqlite"),
        &directory.path().join("covers"),
    )
    .unwrap();
    let database = runtime.database.clone();
    prepared_empty_library(&root_path);
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
        .authorize_sync_peer(binding, peer, "Peer".into(), "secure-store://peer".into())
        .unwrap();

    let bytes = b"peer uploaded audio";
    let song = echo_core::domain::ids::SongId::new();
    let relative_path = format!("media/{song}.flac");
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".into(),
        &relative_path,
    )
    .unwrap();
    let candidate = MutationCandidate {
        resource_refs: vec![manifest.clone()],
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": song,
            "relative_path": manifest.relative_path.as_str(),
            "title": "Uploaded",
            "artist": "Peer",
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
        .unwrap();
    database.mark_published(checkpoint).unwrap();

    let store = Arc::new(
        FileResourceStore::new(
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join("private-resources"),
        )
        .unwrap(),
    );
    let owner = DeviceId::from_uuid(database.current_device_id().as_uuid());
    store
        .import_reader(
            owner,
            &PrepareResource {
                binding,
                resource: ResourceRefDto::from(&manifest),
            },
            &mut std::io::Cursor::new(bytes),
        )
        .unwrap();
    let registry = RootRegistry::new();
    registry.register(root, &root_path);
    let publication = RemoteResourcePublishDeps {
        fs: Arc::new(RootConstrainedFileSystem::new(registry.clone())),
        hasher: Arc::new(Blake3ContentHasher::new(registry)),
        journal: database.clone(),
    };
    let sets = database.pending_peer_resource_sets(root).unwrap();
    assert_eq!(sets.len(), 1);
    publish_remote_resource_set(
        publication,
        &database,
        &store,
        binding,
        root,
        sets[0].0,
        &sets[0].1,
    )
    .unwrap();

    let published = database
        .local_remote_resource_path(root, ObjectKey::Song(song), &manifest)
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(root_path.join(published.normalized())).unwrap(),
        bytes
    );
    assert!(database
        .remote_resource_media_ready(root, ObjectKey::Song(song))
        .unwrap());
    assert!(
        RemoteResourceJournalPort::incomplete(database.as_ref(), root)
            .unwrap()
            .is_empty()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn enrollment_snapshot_is_frozen_and_incremental_pull_after_h_converges_once() {
    let directory = tempfile::tempdir().unwrap();
    let host_runtime = echo_desktop::runtime::app::assemble(
        &directory.path().join("host.sqlite"),
        &directory.path().join("host-covers"),
    )
    .unwrap();
    let replica_runtime = echo_desktop::runtime::app::assemble(
        &directory.path().join("replica.sqlite"),
        &directory.path().join("replica-covers"),
    )
    .unwrap();
    let host = host_runtime.database.as_ref();
    let replica = replica_runtime.database.as_ref();
    let host_root = LibraryRootId::new();
    let replica_root = LibraryRootId::new();
    for (runtime, root) in [(&host_runtime, host_root), (&replica_runtime, replica_root)] {
        let path = directory.path().join(root.to_string());
        std::fs::create_dir(&path).unwrap();
        prepared_empty_library(&path);
        runtime.registry.register(root, &path);
        echo_core::application::ports::LibraryRepository::upsert(
            runtime.database.as_ref(),
            &LibraryRoot::new(root, path, true, true),
        )
        .unwrap();
    }
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, host_root).unwrap();
    replica
        .bind_sync_library(binding, false, replica_root)
        .unwrap();
    let audio = |bytes: u8, path: String| {
        SharedResourceManifest::new(
            SharedResourceKind::Audio,
            ResourceDigest::from_bytes([bytes; 32]),
            4,
            "audio/flac".to_owned(),
            &path,
        )
        .unwrap()
    };
    let first_song = echo_core::domain::ids::SongId::new();
    let first_resource = audio(1, format!("media/{first_song}.flac"));
    let (_, first_mutation) =
        publish_test_song(host, binding, first_song, 1, None, vec![first_resource]);
    let second_song = echo_core::domain::ids::SongId::new();
    let (frozen_head, second_mutation) = publish_test_song(
        host,
        binding,
        second_song,
        2,
        Some(first_mutation),
        Vec::new(),
    );

    // Enrollment captures H and begins downloading its frozen two-record view.
    // The host then commits beyond H while the replica is between pages.
    let session = host.create_host_snapshot(binding, 1, 60_000).unwrap();
    assert_eq!(session.checkpoint, frozen_head);
    let first_page = host.host_snapshot_page(binding, &session.token, 0).unwrap();
    let peer = DeviceId::new();
    let first_apply = replica
        .apply_snapshot_page(
            peer,
            &ReplicaSnapshotPage {
                snapshot_token: session.token.clone(),
                checkpoint: session.checkpoint,
                page_index: first_page.page_index,
                next_page_index: first_page.next_page_index,
                records: first_page.records,
            },
        )
        .unwrap();
    assert_eq!(first_apply.checkpoint, None);

    let third_song = echo_core::domain::ids::SongId::new();
    let third_resource = audio(3, format!("media/{third_song}.flac"));
    let (beyond_h, _) = publish_test_song(
        host,
        binding,
        third_song,
        3,
        Some(second_mutation),
        vec![third_resource],
    );

    let last_page = host.host_snapshot_page(binding, &session.token, 1).unwrap();
    assert_eq!(last_page.next_page_index, None);
    assert!(last_page.records.iter().all(|record| {
        !matches!(record, echo_core::application::ports::HostSnapshotRecord::Object { candidate, .. }
            if candidate.object == ObjectKey::Song(third_song))
    }));
    let snapshot_applied = replica
        .apply_snapshot_page(
            peer,
            &ReplicaSnapshotPage {
                snapshot_token: session.token,
                checkpoint: session.checkpoint,
                page_index: last_page.page_index,
                next_page_index: last_page.next_page_index,
                records: last_page.records,
            },
        )
        .unwrap();
    assert_eq!(snapshot_applied.checkpoint, Some(frozen_head));

    // Pull the log strictly after H, as the provisioner does after enrollment.
    let entries = host
        .published_commits_after(binding, frozen_head.sequence, frozen_head.digest, 512)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].sequence, beyond_h.sequence);
    let commits = entries
        .into_iter()
        .map(|entry| {
            let body: serde_json::Value = serde_json::from_str(&entry.canonical_json).unwrap();
            let mutation = &body["mutation"];
            RemoteCommit {
                sequence: entry.sequence,
                previous_digest: entry.previous_digest,
                digest: entry.digest,
                origin_device_id: serde_json::from_value(mutation["origin_device_id"].clone())
                    .unwrap(),
                mutation_id: serde_json::from_value(mutation["mutation_id"].clone()).unwrap(),
                object: serde_json::from_value(mutation["object"].clone()).unwrap(),
                payload_json: serde_json::to_string(&mutation["payload"]).unwrap(),
                canonical_json: entry.canonical_json,
                is_tombstone: false,
            }
        })
        .collect::<Vec<_>>();
    let incremental_body: serde_json::Value =
        serde_json::from_str(&commits[0].canonical_json).unwrap();
    assert_eq!(
        incremental_body["mutation"]["resource_refs"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "incremental song commit must carry its single immutable resource reference"
    );
    let cursor = replica
        .apply_remote_batch(binding, peer, frozen_head, &commits)
        .unwrap();
    assert_eq!(cursor.sequence, beyond_h.sequence);
    assert_eq!(cursor.digest, beyond_h.digest);

    let songs = replica.all_in_root(replica_root).unwrap();
    let song_ids: std::collections::HashSet<_> = songs
        .iter()
        .map(echo_core::domain::entities::Song::id)
        .collect();
    assert_eq!(songs.len(), 3);
    assert_eq!(
        song_ids.len(),
        3,
        "incremental apply must not duplicate UUIDs"
    );
    assert_eq!(
        replica
            .apply_remote_batch(
                binding,
                peer,
                CommitCheckpoint {
                    binding,
                    sequence: cursor.sequence,
                    digest: cursor.digest,
                },
                &[],
            )
            .unwrap()
            .sequence,
        cursor.sequence
    );
}

/// A host publication checkpoint that observes the replica at the first upload.
/// This catches push-before-pull even when the final catalogs would converge.
struct ObservePullBeforePush {
    replica: Arc<SqliteDatabase>,
    host_song: echo_core::domain::ids::SongId,
    observed: AtomicBool,
}

impl echo_core::application::ports::PublicationCheckpointPort for ObservePullBeforePush {
    fn advance(&self, _checkpoint: CommitCheckpoint) -> Result<(), echo_core::error::Error> {
        assert!(
            SongRepository::by_id(self.replica.as_ref(), self.host_song)?.is_some(),
            "host song must already be merged before the peer publishes local additions"
        );
        self.observed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// Counts host-side `prepare` requests, so a test can prove the replica stops
/// re-declaring resources whose bytes the host already confirmed.
struct CountingResources {
    inner: FileResourceStore,
    prepares: Arc<std::sync::atomic::AtomicUsize>,
}

impl ResourceTransferPort for CountingResources {
    fn prepare(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<TransferProgress, TransferError> {
        self.prepares.fetch_add(1, Ordering::Relaxed);
        self.inner.prepare(owner, request)
    }

    fn write_chunk(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
        index: u32,
        hash: &str,
        bytes: &[u8],
    ) -> Result<TransferProgress, TransferError> {
        self.inner
            .write_chunk(binding, owner, upload, index, hash, bytes)
    }

    fn complete(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<TransferProgress, TransferError> {
        self.inner.complete(binding, owner, upload)
    }

    fn verify_complete(
        &self,
        binding: SyncBinding,
        resource: &ResourceRefDto,
    ) -> Result<(), TransferError> {
        self.inner.verify_complete(binding, resource)
    }

    fn download(
        &self,
        binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<DownloadChunk, TransferError> {
        self.inner.download(binding, hash, offset, length)
    }
}

/// Counts host-side commit-page reads so a test can bound what one idle cycle
/// costs the host's request budget.
struct CountingLedger {
    inner: Arc<SqliteDatabase>,
    pulls: Arc<std::sync::atomic::AtomicUsize>,
}

impl HostCommitLedgerPort for CountingLedger {
    fn stage_confirmation(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<echo_core::application::ports::StagedConfirmation, echo_core::error::Error> {
        self.inner.stage_confirmation(binding, candidate)
    }

    fn mark_published(&self, checkpoint: CommitCheckpoint) -> Result<(), echo_core::error::Error> {
        self.inner.mark_published(checkpoint)
    }

    fn pending_publications(
        &self,
        limit: usize,
    ) -> Result<Vec<echo_core::application::ports::StagedConfirmation>, echo_core::error::Error>
    {
        self.inner.pending_publications(limit)
    }

    fn published_commits_after(
        &self,
        binding: SyncBinding,
        after: CommitSeq,
        expected_digest: [u8; 32],
        limit: usize,
    ) -> Result<Vec<echo_core::application::ports::PublishedCommit>, echo_core::error::Error> {
        self.pulls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .published_commits_after(binding, after, expected_digest, limit)
    }
}

struct LanMergeFixture {
    directory: tempfile::TempDir,
    host: Arc<SqliteDatabase>,
    replica: Arc<SqliteDatabase>,
    root: LibraryRootId,
    host_root: LibraryRootId,
    host_song: echo_core::domain::ids::SongId,
    host_mutation: MutationId,
    provisioner: NativeSyncProvisioner,
    client: LanClient,
    peer: ProvisionedPeer,
    endpoint: Url,
    resources: Arc<FileResourceStore>,
    host_prepares: Arc<std::sync::atomic::AtomicUsize>,
    host_pulls: Arc<std::sync::atomic::AtomicUsize>,
    checkpoint: Arc<ObservePullBeforePush>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Drop for LanMergeFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl LanMergeFixture {
    #[allow(clippy::too_many_lines)]
    async fn new() -> Self {
        use echo_core::infrastructure::filesystem::{
            Blake3ContentHasher, RootConstrainedFileSystem, RootRegistry,
        };
        let directory = tempfile::tempdir().unwrap();
        let host_runtime = echo_desktop::runtime::app::assemble(
            &directory.path().join("host.sqlite"),
            &directory.path().join("host-covers"),
        )
        .unwrap();
        let host = host_runtime.database.clone();
        let replica_runtime = echo_desktop::runtime::app::assemble(
            &directory.path().join("replica.sqlite"),
            &directory.path().join("covers"),
        )
        .unwrap();
        let replica = replica_runtime.database.clone();
        let binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        let mut roots = Vec::new();
        for (database, name, is_host) in [(&host, "host", true), (&replica, "peer", false)] {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.join("media")).unwrap();
            prepared_empty_library(&path);
            let root = derive_root_id(&path.canonicalize().unwrap());
            if is_host {
                host_runtime.registry.register(root, &path);
            } else {
                replica_runtime.registry.register(root, &path);
            }
            LibraryRepository::upsert(database.as_ref(), &LibraryRoot::new(root, path, true, true))
                .unwrap();
            if is_host {
                database.bind_sync_library(binding, true, root).unwrap();
            } else {
                database
                    .bind_sync_replica_for_enrollment(binding, root)
                    .unwrap();
            }
            roots.push(root);
        }
        let host_root = roots[0];
        let root = roots[1];
        let host_song = echo_core::domain::ids::SongId::new();
        let (_, host_mutation) = publish_test_song(&host, binding, host_song, 1, None, vec![]);
        let checkpoint = Arc::new(ObservePullBeforePush {
            replica: replica.clone(),
            host_song,
            observed: AtomicBool::new(false),
        });
        let peer_device = DeviceId::from_uuid(replica.current_device_id().as_uuid());
        host.authorize_sync_peer(binding, peer_device, "Test peer".into(), "test-only".into())
            .unwrap();
        let token = "temporary-loopback-test-token-123456789";
        let host_prepares = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let host_pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = HostApiState::new(
            binding,
            vec![PeerCredential {
                device_id: peer_device,
                bearer_token: token.as_bytes().to_vec(),
            }],
            Arc::new(CountingLedger {
                inner: Arc::clone(&host),
                pulls: Arc::clone(&host_pulls),
            }),
            checkpoint.clone(),
        )
        .unwrap()
        .with_authorization(host.clone())
        .with_snapshots(host.clone())
        .with_resources(Arc::new(CountingResources {
            inner: FileResourceStore::new(
                directory
                    .path()
                    .canonicalize()
                    .unwrap()
                    .join("host-resources"),
            )
            .unwrap(),
            prepares: Arc::clone(&host_prepares),
        }));
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let certificate_der = certificate.cert.der().to_vec();
        let tls = tls13_server_config(
            vec![CertificateDer::from(certificate_der.clone())],
            PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der()).into(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(echo_sync::server::serve_tls(
            listener,
            tls,
            echo_sync::server::host_router(api),
        ));
        let peer = ProvisionedPeer {
            binding,
            address: endpoint.to_string(),
            host_device_id: DeviceId::from_uuid(host.current_device_id().as_uuid()),
        };
        let registry = RootRegistry::new();
        registry.register(root, directory.path().join("peer"));
        replica_runtime
            .registry
            .register(root, directory.path().join("peer"));
        let publication = RemoteResourcePublishDeps {
            fs: Arc::new(RootConstrainedFileSystem::new(registry.clone())),
            hasher: Arc::new(Blake3ContentHasher::new(registry)),
            journal: replica.clone(),
        };
        let provisioner = NativeSyncProvisioner::new(
            replica.clone(),
            directory.path().join("private"),
            publication,
            replica_runtime.deps,
            ScanSupervisor::new(),
        );
        let resources = Arc::new(
            FileResourceStore::new(directory.path().canonicalize().unwrap().join("resources"))
                .unwrap(),
        );
        Self {
            directory,
            host,
            replica,
            root,
            host_root,
            host_song,
            host_mutation,
            provisioner,
            client: LanClient::new(&certificate_der, token.into()).unwrap(),
            peer,
            endpoint,
            resources,
            host_prepares,
            host_pulls,
            checkpoint,
            server,
        }
    }
}

#[path = "tests/embedded_resources.rs"]
mod embedded_resources;
#[path = "tests/merge_cycle.rs"]
mod merge_cycle;
#[path = "tests/queue_authorization.rs"]
mod queue_authorization;

#[path = "tests/host_publication.rs"]
mod host_publication;

#[path = "tests/missing_publications.rs"]
mod missing_publications;

#[path = "tests/cache_cleanup.rs"]
mod cache_cleanup;
