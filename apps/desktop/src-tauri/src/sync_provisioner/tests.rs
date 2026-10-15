use super::*;
use echo_core::domain::sync::{CommitSeq, ResourceDigest};

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
fn audio_seed_refuses_missing_or_replaced_frozen_bytes() {
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
        Err(SyncRuntimeError::LibraryUnavailable)
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
    database
        .upsert(&LibraryRoot::new(
            root,
            directory.path().to_path_buf(),
            true,
            true,
        ))
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
    assert!(matches!(
        pairing.approve(&request),
        Err(PairingError::Unavailable)
    ));
    assert!(database
        .sync_peer_has_credential(binding, device_id, "prior-generation".to_owned())
        .unwrap());
}
