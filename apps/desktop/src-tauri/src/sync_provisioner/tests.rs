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
    std::fs::write(root.join("echo/manifest.json"), b"{}").unwrap();
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
    let database = Arc::new(SqliteDatabase::open(directory.path().join("host.sqlite")).unwrap());
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
    let host = SqliteDatabase::open(directory.path().join("host.sqlite")).unwrap();
    let replica = SqliteDatabase::open(directory.path().join("replica.sqlite")).unwrap();
    let host_root = LibraryRootId::new();
    let replica_root = LibraryRootId::new();
    for (database, root) in [(&host, host_root), (&replica, replica_root)] {
        echo_core::application::ports::LibraryRepository::upsert(
            database,
            &LibraryRoot::new(root, directory.path().join(root.to_string()), true, true),
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
        publish_test_song(&host, binding, first_song, 1, None, vec![first_resource]);
    let second_song = echo_core::domain::ids::SongId::new();
    let (frozen_head, second_mutation) = publish_test_song(
        &host,
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
        &host,
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
        let host = Arc::new(SqliteDatabase::open(directory.path().join("host.sqlite")).unwrap());
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
            client: LanClient::new(&certificate_der, token.into()).unwrap(),
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_merge_pulls_host_before_uploading_and_preserves_local_song() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let local_song = SongId::new();
    let mut song = Song::new(
        local_song,
        fixture.root,
        RelativeMediaPath::new("media/local.flac").unwrap(),
        Revision::INITIAL,
    );
    let bytes = b"keep local audio";
    song.apply_scan_facts(
        blake3::hash(bytes).to_hex().to_string(),
        bytes.len() as u64,
        1,
        echo_core::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(fixture.replica.as_ref(), &song).unwrap();
    let sentinel = fixture.directory.path().join("peer/media/local.flac");
    std::fs::write(&sentinel, b"keep local audio").unwrap();

    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    for manifest in fixture
        .replica
        .local_sync_resource_manifests(fixture.root)
        .unwrap()
    {
        fixture
            .resources
            .import_reader(
                DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid()),
                &PrepareResource {
                    binding: fixture.peer.binding,
                    resource: ResourceRefDto::from(&manifest),
                },
                &mut std::io::Cursor::new(bytes),
            )
            .unwrap();
    }
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();

    assert!(fixture.checkpoint.observed.load(Ordering::Relaxed));
    for (database, root) in [
        (&fixture.host, fixture.host_root),
        (&fixture.replica, fixture.root),
    ] {
        let ids = database
            .all_in_root(root)
            .unwrap()
            .iter()
            .map(echo_core::domain::entities::Song::id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            ids,
            std::collections::HashSet::from([fixture.host_song, local_song])
        );
    }
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep local audio");
    let host_resources = FileResourceStore::new(
        fixture
            .directory
            .path()
            .canonicalize()
            .unwrap()
            .join("host-resources"),
    )
    .unwrap();
    assert_eq!(
        host_resources
            .download(
                fixture.peer.binding,
                blake3::hash(bytes).to_hex().as_ref(),
                0,
                bytes.len()
            )
            .unwrap()
            .bytes,
        bytes
    );
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host.all_in_root(fixture.host_root).unwrap().len(),
        2
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"keep local audio");
}

#[tokio::test]
/// A steady idle cycle must reach the host once, not three times. The catch-up
/// pull that used to open every cycle was redundant with the pull inside
/// `synchronize_once`, and the closing pull only exists for cycles that pushed
/// something. Empty pages are free individually and expensive in aggregate:
/// this is the per-cycle tax a fallback (5 s) peer paid 12 times a minute.
async fn an_idle_cycle_reaches_the_host_exactly_once() {
    let fixture = LanMergeFixture::new().await;
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert!(
        SongRepository::by_id(fixture.replica.as_ref(), fixture.host_song)
            .unwrap()
            .is_some(),
        "dropping the opening pull must not drop the host's commits"
    );
    let after_warmup = fixture.host_pulls.load(Ordering::Relaxed);
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host_pulls.load(Ordering::Relaxed) - after_warmup,
        1,
        "a caught-up cycle reads one commit page, not one per phase"
    );
}

#[tokio::test]
async fn native_pull_drains_more_than_one_commit_page_without_losing_local_work() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let local_song = SongId::new();
    SongRepository::upsert(
        fixture.replica.as_ref(),
        &Song::new(
            local_song,
            fixture.root,
            RelativeMediaPath::new("media/local.flac").unwrap(),
            Revision::INITIAL,
        ),
    )
    .unwrap();
    let queued = fixture.replica.pending_local_mutations(512).unwrap();
    let mut previous = fixture.host_mutation;
    let mut last = None;
    for sequence in 2..=513 {
        let (checkpoint, mutation) = publish_test_song(
            &fixture.host,
            fixture.peer.binding,
            SongId::new(),
            sequence,
            Some(previous),
            vec![],
        );
        previous = mutation;
        last = Some(checkpoint);
    }
    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    fixture
        .provisioner
        .pull_host_changes(&fixture.peer, &fixture.endpoint, &queue)
        .await
        .unwrap();
    assert_eq!(
        queue
            .replica_checkpoint(fixture.peer.host_device_id)
            .unwrap(),
        last.unwrap()
    );
    assert_eq!(
        fixture.replica.all_in_root(fixture.root).unwrap().len(),
        514
    );
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap(),
        queued
    );
}

#[tokio::test]
#[ignore = "real LAN host rate limit makes 513 uploads take several minutes"]
async fn native_merge_drains_more_than_512_local_additions() {
    use echo_core::application::ports::PlaylistRepository;
    use echo_core::domain::ids::PlaylistId;
    let fixture = LanMergeFixture::new().await;
    // Pair an empty replica first, then add more than one upload page locally.
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    for index in 0..513 {
        PlaylistRepository::create(
            fixture.replica.as_ref(),
            PlaylistId::new(),
            fixture.root,
            &format!("Local playlist {index}"),
        )
        .unwrap();
    }
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap().len(),
        512
    );
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());
    assert_eq!(
        fixture.host.all_in_root(fixture.host_root).unwrap().len(),
        1
    );
    assert!(fixture.checkpoint.observed.load(Ordering::Relaxed));
    assert_eq!(
        PlaylistRepository::list(fixture.host.as_ref(), fixture.host_root)
            .unwrap()
            .len(),
        513
    );
    assert_eq!(
        PlaylistRepository::list(fixture.replica.as_ref(), fixture.root)
            .unwrap()
            .len(),
        513
    );
}

#[tokio::test]
async fn native_enrollment_persists_host_manifests_before_downloading_audio() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    let song = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(b"not seeded on host").as_bytes()),
        18,
        "audio/flac".into(),
        &format!("media/{song}.flac"),
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        song,
        2,
        Some(fixture.host_mutation),
        vec![manifest.clone()],
    );

    // The host resource store deliberately lacks these bytes. Catalog enrollment
    // must finish first so a later media transfer can resume from durable state.
    fixture
        .provisioner
        .enroll_peer_snapshot(&fixture.peer, &fixture.endpoint)
        .await
        .unwrap();

    assert!(fixture
        .replica
        .replica_enrollment_complete(fixture.peer.host_device_id)
        .unwrap());
    let sets = fixture
        .replica
        .pending_remote_resource_sets(fixture.root)
        .unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].0, ObjectKey::Song(song));
    assert_eq!(sets[0].1[0].manifest, manifest);
    assert!(fixture
        .resources
        .verify_complete(fixture.peer.binding, &ResourceRefDto::from(&manifest))
        .is_err());
}

/// An incremental commit page must land its catalog objects even when the host
/// cannot serve the referenced bytes yet. Media is attached afterwards from the
/// durable manifests, exactly like snapshot enrollment, so a stalled transfer
/// cannot park song and playlist updates behind it.
#[tokio::test]
async fn native_incremental_applies_objects_before_audio_arrives() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    let song = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(b"incremental bytes not on host").as_bytes()),
        18,
        "audio/flac".into(),
        &format!("media/{song}.flac"),
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        song,
        2,
        Some(fixture.host_mutation),
        vec![manifest.clone()],
    );

    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    let pulled = tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .provisioner
            .pull_host_changes(&fixture.peer, &fixture.endpoint, &queue),
    )
    .await
    .expect("object application must not wait for the media transfer");
    pulled.unwrap();

    // The object exists, is explicitly unavailable rather than absent, and its
    // verified bytes are still outstanding for publication to attach later.
    let projected = SongRepository::by_id(fixture.replica.as_ref(), song)
        .unwrap()
        .expect("incremental page must materialize the song without its bytes");
    assert!(
        !projected.availability().is_playable(),
        "a confirmed remote song without its bytes must stay unavailable"
    );
    let sets = fixture
        .replica
        .pending_remote_resource_sets(fixture.root)
        .unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].0, ObjectKey::Song(song));
    assert_eq!(sets[0].1[0].manifest, manifest);
    assert!(fixture
        .resources
        .verify_complete(fixture.peer.binding, &ResourceRefDto::from(&manifest))
        .is_err());
}

/// A resource is prepared once per session. The unacked mutation keeps
/// declaring it every cycle, so without the session cache a 512-deep backlog
/// re-sends `prepare` for the whole queue each round and re-reads each blob
/// twice to re-derive BLAKE3.
#[tokio::test]
async fn native_upload_prepares_a_resource_once_until_the_host_acks_it() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let song = Song::new(
        SongId::new(),
        fixture.root,
        RelativeMediaPath::new("media/upload.flac").unwrap(),
        Revision::INITIAL,
    );
    let bytes = b"uploadable audio bytes";
    let mut song = song;
    song.apply_scan_facts(
        blake3::hash(bytes).to_hex().to_string(),
        bytes.len() as u64,
        1,
        echo_core::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(fixture.replica.as_ref(), &song).unwrap();
    std::fs::write(
        fixture.directory.path().join("peer/media/upload.flac"),
        bytes,
    )
    .unwrap();
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    let device = DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid());
    for manifest in fixture
        .replica
        .local_sync_resource_manifests(fixture.root)
        .unwrap()
    {
        fixture
            .resources
            .import_reader(
                device,
                &PrepareResource {
                    binding: fixture.peer.binding,
                    resource: ResourceRefDto::from(&manifest),
                },
                &mut std::io::Cursor::new(bytes),
            )
            .unwrap();
    }
    let pending = fixture.replica.pending_local_mutations(512).unwrap();
    assert!(!pending.is_empty());
    assert!(pending
        .iter()
        .flat_map(|mutation| &mutation.resource_refs)
        .any(|resource| resource.kind == SharedResourceKind::Audio));
    // The baseline enqueues one mutation per local projection, so several of
    // them re-declare the same audio resource. Each distinct resource is
    // prepared exactly once.
    let distinct: std::collections::HashSet<_> = pending
        .iter()
        .flat_map(|mutation| &mutation.resource_refs)
        .map(|resource| resource.digest.to_hex())
        .collect();
    assert!(!distinct.is_empty());

    fixture
        .provisioner
        .upload_pending_resources(&fixture.peer, &fixture.endpoint, fixture.root)
        .await
        .unwrap();
    assert_eq!(
        fixture.host_prepares.load(Ordering::Relaxed),
        distinct.len()
    );

    // The host has not acknowledged the mutation, so the next cycle selects the
    // same resource again; it must not cost another request or another hash.
    fixture
        .provisioner
        .upload_pending_resources(&fixture.peer, &fixture.endpoint, fixture.root)
        .await
        .unwrap();
    assert_eq!(fixture.host_prepares.load(Ordering::Relaxed), 1);
}

/// Pull until the replica sits at the host's head, so the next commit page is
/// empty and the host holds it open for the watch under test.
async fn pull_replica_to_host_head(fixture: &LanMergeFixture) {
    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    fixture
        .provisioner
        .pull_host_changes(&fixture.peer, &fixture.endpoint, &queue)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_host_publication_ends_a_held_watch() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    pull_replica_to_host_head(&fixture).await;

    let cancelled = Arc::new(AtomicBool::new(false));
    // Records when the host's publication committed, so the assertion below
    // measures propagation latency rather than the test's own settle delay.
    let published_at = Arc::new(Mutex::new(None::<Instant>));
    let joined = tokio::time::timeout(Duration::from_secs(5), async {
        let published_at = Arc::clone(&published_at);
        tokio::join!(
            async {
                // Let the watch settle into the held page before publishing.
                tokio::time::sleep(Duration::from_millis(300)).await;
                let committed = Instant::now();
                publish_test_song(
                    &fixture.host,
                    fixture.peer.binding,
                    SongId::new(),
                    2,
                    Some(fixture.host_mutation),
                    vec![],
                );
                *published_at
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(committed);
            },
            fixture.provisioner.watch_peer_commits(
                &fixture.peer,
                fixture.root,
                cancelled,
                Duration::from_secs(30),
            ),
        )
    })
    .await;
    let ((), outcome) = joined.expect("a held watch must end when the host publishes");
    assert_eq!(outcome.unwrap(), WatchOutcome::Advanced);
    // The host rechecks the held page every 500 ms, so the peer learns about a
    // publication in about one second — the plain-poll floor was five.
    let latency = published_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("the publication committed")
        .elapsed();
    assert!(
        latency < Duration::from_secs(2),
        "the held watch took {latency:?} to notice the publication"
    );
}

#[tokio::test]
async fn a_local_write_ends_a_held_watch() {
    use echo_core::application::ports::PlaylistRepository;
    use echo_core::domain::ids::PlaylistId;
    let fixture = LanMergeFixture::new().await;
    pull_replica_to_host_head(&fixture).await;

    let cancelled = Arc::new(AtomicBool::new(false));
    let joined = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                // A local write wakes every sync listener, including the
                // channel this provisioner registered for its watch.
                PlaylistRepository::create(
                    fixture.replica.as_ref(),
                    PlaylistId::new(),
                    fixture.root,
                    "Local edit",
                )
                .unwrap();
            },
            fixture.provisioner.watch_peer_commits(
                &fixture.peer,
                fixture.root,
                cancelled,
                Duration::from_secs(30),
            ),
        )
    })
    .await;
    let ((), outcome) = joined.expect("a held watch must end on a local write");
    assert_eq!(outcome.unwrap(), WatchOutcome::LocalWrite);
}

#[derive(serde::Deserialize)]
struct ImmediateCommitQuery {
    after: u64,
    previous_digest: String,
}

/// A host from before the bounded long-poll: it answers an empty page at once
/// and ignores `wait_ms` entirely.
async fn immediate_commit_page(
    axum::extract::Query(query): axum::extract::Query<ImmediateCommitQuery>,
) -> axum::Json<CommitPageDto> {
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&query.previous_digest[index * 2..index * 2 + 2], 16).unwrap();
    }
    axum::Json(CommitPageDto {
        binding: SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        },
        anchor_sequence: CommitSeq::from_u64(query.after),
        anchor_digest: digest,
        entries: Vec::new(),
    })
}

/// Serve an immediate-answer host over TLS and return its endpoint, pinned
/// certificate, and server handle.
async fn spawn_immediate_commit_host(
) -> (Url, Vec<u8>, tokio::task::JoinHandle<std::io::Result<()>>) {
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let certificate_der = certificate.cert.der().to_vec();
    let tls = tls13_server_config(
        vec![CertificateDer::from(certificate_der.clone())],
        PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der()).into(),
    )
    .unwrap();
    let router =
        axum::Router::new().route("/v1/commits", axum::routing::get(immediate_commit_page));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(echo_sync::server::serve_tls(listener, tls, router));
    (endpoint, certificate_der, server)
}

#[tokio::test]
async fn a_host_that_answers_before_the_budget_is_reported_as_unsupported() {
    let fixture = LanMergeFixture::new().await;
    let (endpoint, certificate_der, server) = spawn_immediate_commit_host().await;
    let peer = ProvisionedPeer {
        binding: fixture.peer.binding,
        address: endpoint.to_string(),
        client: LanClient::new(
            &certificate_der,
            "temporary-loopback-test-token-123456789".into(),
        )
        .unwrap(),
        host_device_id: DeviceId::new(),
    };

    let cancelled = Arc::new(AtomicBool::new(false));
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.provisioner.watch_peer_commits(
            &peer,
            fixture.root,
            cancelled,
            Duration::from_secs(30),
        ),
    )
    .await;
    server.abort();
    let outcome = outcome.expect("an immediate host must end the watch at once");
    assert_eq!(
        outcome.unwrap(),
        WatchOutcome::Unsupported,
        "an early empty page means the host cannot hold; the caller must poll instead"
    );
}
