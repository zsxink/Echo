use std::sync::Arc;

use echo_core::application::ports::{
    ControlPlanePort, DeviceIdProvider, HostCommitLedgerPort, LibraryRepository,
};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{LibraryRootId, SongId};
use echo_core::domain::library::LibraryManifest;
#[cfg(feature = "client")]
use echo_core::domain::sync::ObjectVersion;
use echo_core::domain::sync::{
    AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
    SyncBinding,
};
use echo_core::infrastructure::filesystem::{RootControlPlane, RootRegistry};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_sync::protocol::{commit_digest, CommitDto, MutationDto, PROTOCOL_VERSION};
#[cfg(feature = "client")]
use echo_sync::{
    client::apply_commit_page,
    protocol::{
        CheckpointDto, CommitPageDto, CommittedEntryDto, SnapshotObjectDto, SnapshotPageDto,
    },
};

#[test]
fn sqlite_host_commit_digest_matches_the_wire_protocol() {
    verify_ledger_protocol(false);
}

#[test]
fn maximum_size_mutation_remains_transportable_after_confirmation() {
    verify_ledger_protocol(true);
}

#[allow(clippy::too_many_lines)]
fn verify_ledger_protocol(maximum_payload: bool) {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("library.db")).unwrap();
    #[cfg(feature = "client")]
    let replica_directory = tempfile::tempdir().unwrap();
    #[cfg(feature = "client")]
    let replica = SqliteDatabase::open(replica_directory.path().join("replica.db")).unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    bind_library_database(&database, &directory.path().join("music"), binding, true);
    #[cfg(feature = "client")]
    bind_library_database(
        &replica,
        &replica_directory.path().join("music"),
        binding,
        false,
    );
    let device = DeviceId::from_uuid(database.current_device_id().as_uuid());
    let song = SongId::new();
    let mut mutation = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": song.to_string(),
            "relative_path": "Music/digest-parity.flac",
            "title": "digest parity",
            "added_at": 1,
        }),
    };
    if maximum_payload {
        pad_to_mutation_limit(binding, &mut mutation);
    }
    let staged = database.stage_confirmation(binding, &mutation).unwrap();
    let checkpoint = staged.checkpoint.unwrap();
    database.mark_published(checkpoint).unwrap();
    let commits = database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 8)
        .unwrap();
    let saved = &commits[0];
    let wire: CommitDto = serde_json::from_str(&saved.canonical_json).unwrap();
    if maximum_payload {
        assert!(saved.canonical_json.len() > echo_sync::protocol::MAX_MUTATION_BYTES);
        assert!(saved.canonical_json.len() <= echo_sync::protocol::MAX_COMMIT_BYTES);
    }
    assert_eq!(
        wire,
        CommitDto {
            protocol_version: PROTOCOL_VERSION,
            binding,
            commit_seq: checkpoint.sequence,
            previous_digest: [0; 32],
            mutation: MutationDto {
                resource_refs: Vec::new(),
                protocol_version: PROTOCOL_VERSION,
                binding,
                origin_device_id: device,
                mutation_id: mutation.mutation_id,
                origin_seq: 1,
                previous_mutation_id: None,
                object: mutation.object,
                base_object_version: None,
                changed_at_ms: None,
                payload: mutation.payload,
            },
            aliases: vec![],
            resource_refs: vec![],
        }
    );
    assert_eq!(commit_digest(&wire).unwrap(), saved.digest);

    #[cfg(feature = "client")]
    let page = CommitPageDto {
        binding,
        anchor_sequence: CommitSeq::GENESIS,
        anchor_digest: [0; 32],
        entries: vec![CommittedEntryDto {
            commit: wire.clone(),
            digest: saved.digest,
        }],
    };
    #[cfg(feature = "client")]
    let applied = apply_commit_page(&page, DeviceId::new(), &replica).unwrap();
    #[cfg(feature = "client")]
    assert_eq!(applied.sequence, checkpoint.sequence);
    #[cfg(feature = "client")]
    assert_eq!(applied.digest, checkpoint.digest);
    #[cfg(feature = "client")]
    assert_eq!(applied.applied, 1);

    #[cfg(feature = "client")]
    {
        verify_snapshot_enrollment(binding, checkpoint, wire.mutation);
    }
}

#[cfg(feature = "client")]
fn verify_snapshot_enrollment(
    binding: SyncBinding,
    checkpoint: echo_core::domain::sync::CommitCheckpoint,
    mutation: MutationDto,
) {
    let enrollment_directory = tempfile::tempdir().unwrap();
    let enrollment = SqliteDatabase::open(enrollment_directory.path().join("library.db")).unwrap();
    bind_library_database(
        &enrollment,
        &enrollment_directory.path().join("music"),
        binding,
        false,
    );
    let snapshot = SnapshotPageDto {
        snapshot_token: MutationId::new().as_uuid().simple().to_string(),
        checkpoint: CheckpointDto {
            binding,
            commit_seq: checkpoint.sequence,
            digest: checkpoint.digest,
        },
        page_index: 0,
        next_page_index: None,
        objects: vec![SnapshotObjectDto {
            mutation,
            version: ObjectVersion::from_commit_seq(checkpoint.sequence),
            resources: vec![],
        }],
        aliases: vec![],
    };
    let applied =
        echo_sync::client::apply_snapshot_page(&snapshot, DeviceId::new(), &enrollment).unwrap();
    assert_eq!(applied.checkpoint, Some(checkpoint));
    assert_eq!(applied.applied, 1);
}

/// Production always attaches a library control plane before sync writes;
/// the directory-first commit path refuses to run without it.
fn bind_library_database(
    database: &SqliteDatabase,
    music_root: &std::path::Path,
    binding: SyncBinding,
    is_host: bool,
) {
    std::fs::create_dir_all(music_root).unwrap();
    let root = LibraryRootId::new();
    let registry = RootRegistry::new();
    registry.register(root, music_root);
    let control = Arc::new(RootControlPlane::new(registry));
    control
        .write_manifest(
            root,
            &LibraryManifest::new(
                echo_core::domain::library::LibraryId::from_uuid(binding.library_id.as_uuid()),
                env!("CARGO_PKG_VERSION"),
            ),
        )
        .unwrap();
    database.configure_library_control(control);
    LibraryRepository::upsert(
        database,
        &LibraryRoot::new(root, music_root.to_path_buf(), true, true),
    )
    .unwrap();
    database.bind_sync_library(binding, is_host, root).unwrap();
}

fn pad_to_mutation_limit(binding: SyncBinding, mutation: &mut MutationCandidate) {
    mutation.payload["padding"] = serde_json::json!("");
    let dto = MutationDto {
        resource_refs: Vec::new(),
        protocol_version: PROTOCOL_VERSION,
        binding,
        origin_device_id: mutation.origin_device_id,
        mutation_id: mutation.mutation_id,
        origin_seq: mutation.origin_seq,
        previous_mutation_id: mutation.previous_mutation_id,
        object: mutation.object,
        base_object_version: mutation.base_object_version,
        changed_at_ms: None,
        payload: mutation.payload.clone(),
    };
    let overhead = serde_json::to_vec(&dto).unwrap().len();
    mutation.payload["padding"] =
        serde_json::json!("x".repeat(echo_sync::protocol::MAX_MUTATION_BYTES - overhead));
    echo_sync::protocol::validate_mutation(&MutationDto {
        payload: mutation.payload.clone(),
        ..dto
    })
    .unwrap();
}
