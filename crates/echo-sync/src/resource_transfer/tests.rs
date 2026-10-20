use super::*;
use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId, SharedResourceKind};

fn fresh_binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(1),
    }
}

fn request(binding: SyncBinding, bytes: &[u8]) -> PrepareResource {
    PrepareResource {
        binding,
        resource: ResourceRefDto {
            kind: SharedResourceKind::Audio,
            content_hash: blake3::hash(bytes).to_hex().to_string(),
            length: bytes.len() as u64,
            media_type: "audio/flac".to_owned(),
            relative_path: "media/example.flac".to_owned(),
        },
    }
}

#[test]
fn chunks_survive_restart_and_publish_only_after_complete_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let owner = DeviceId::new();
    let binding = fresh_binding();
    let bytes = vec![42; MAX_RESOURCE_CHUNK_BYTES + 3];
    let request = request(binding, &bytes);
    let store = FileResourceStore::new(root.clone()).unwrap();
    let progress = store.prepare(owner, &request).unwrap();
    assert!(matches!(
        store.download(binding, &request.resource.content_hash, 0, 1),
        Err(TransferError::Unknown)
    ));
    let first = &bytes[..MAX_RESOURCE_CHUNK_BYTES];
    let hash = blake3::hash(first).to_hex().to_string();
    let progress = store
        .write_chunk(binding, owner, &progress.upload_id, 0, &hash, first)
        .unwrap();
    assert!(matches!(
        store.complete(binding, owner, &progress.upload_id),
        Err(TransferError::Conflict)
    ));
    drop(store);
    let store = FileResourceStore::new(root).unwrap();
    assert_eq!(
        store.prepare(owner, &request).unwrap().confirmed_chunks,
        vec![0]
    );
    assert!(matches!(
        store.write_chunk(
            binding,
            DeviceId::new(),
            &progress.upload_id,
            0,
            &hash,
            first
        ),
        Err(TransferError::Unknown)
    ));
    assert!(matches!(
        store.write_chunk(fresh_binding(), owner, &progress.upload_id, 0, &hash, first),
        Err(TransferError::Unknown)
    ));
    assert_eq!(
        store
            .write_chunk(binding, owner, &progress.upload_id, 0, &hash, first)
            .unwrap()
            .confirmed_chunks,
        vec![0]
    );
    let last = &bytes[MAX_RESOURCE_CHUNK_BYTES..];
    store
        .write_chunk(
            binding,
            owner,
            &progress.upload_id,
            1,
            blake3::hash(last).to_hex().as_str(),
            last,
        )
        .unwrap();
    assert!(
        store
            .complete(binding, owner, &progress.upload_id)
            .unwrap()
            .complete
    );
    assert!(
        store
            .complete(binding, owner, &progress.upload_id)
            .unwrap()
            .complete
    );
    let downloaded = store
        .download(
            binding,
            &request.resource.content_hash,
            MAX_RESOURCE_CHUNK_BYTES as u64,
            3,
        )
        .unwrap();
    assert_eq!(downloaded.bytes, last);
    assert_eq!(downloaded.total_length, bytes.len() as u64);
    assert!(matches!(
        store.download(
            binding,
            &request.resource.content_hash,
            0,
            MAX_RESOURCE_CHUNK_BYTES + 1
        ),
        Err(TransferError::Invalid)
    ));
}

#[test]
fn wrong_complete_hash_and_retried_different_bytes_never_publish() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let request = request(fresh_binding(), b"expected");
    let progress = store.prepare(owner, &request).unwrap();
    let wrong = b"modified";
    store
        .write_chunk(
            request.binding,
            owner,
            &progress.upload_id,
            0,
            blake3::hash(wrong).to_hex().as_str(),
            wrong,
        )
        .unwrap();
    assert!(matches!(
        store.write_chunk(
            request.binding,
            owner,
            &progress.upload_id,
            0,
            blake3::hash(b"expected").to_hex().as_str(),
            b"expected"
        ),
        Err(TransferError::Conflict)
    ));
    assert!(matches!(
        store.complete(request.binding, owner, &progress.upload_id),
        Err(TransferError::Conflict)
    ));
    assert!(matches!(
        store.download(request.binding, &request.resource.content_hash, 0, 8),
        Err(TransferError::Unknown)
    ));
}

#[test]
fn immutable_duplicates_and_empty_resources_are_valid() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let request = request(fresh_binding(), b"content");
    let mut reader = &b"content"[..];
    assert!(
        store
            .import_reader(owner, &request, &mut reader)
            .unwrap()
            .complete
    );
    let reused = store.prepare(DeviceId::new(), &request).unwrap();
    assert!(reused.complete);
    assert!(matches!(
        store.download(fresh_binding(), &request.resource.content_hash, 0, 7),
        Err(TransferError::Unknown)
    ));
    let empty = super::tests::request(request.binding, b"");
    let progress = store.prepare(owner, &empty).unwrap();
    assert!(
        store
            .complete(request.binding, owner, &progress.upload_id)
            .unwrap()
            .complete
    );
}

#[test]
fn cleanup_completed_reclaims_consumed_blobs_but_keeps_incomplete_uploads() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let binding = fresh_binding();
    let completed = request(binding, b"published");
    let mut reader = &b"published"[..];
    store.import_reader(owner, &completed, &mut reader).unwrap();
    let incomplete = request(binding, b"still uploading");
    store.prepare(owner, &incomplete).unwrap();

    store.cleanup_completed(binding).unwrap();

    assert!(matches!(
        store.download(binding, &completed.resource.content_hash, 0, 1),
        Err(TransferError::Unknown)
    ));
    assert!(store
        .prepare(owner, &incomplete)
        .unwrap()
        .confirmed_chunks
        .is_empty());
}

#[test]
fn owner_cleanup_and_detached_binding_cleanup_are_scoped() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let binding = fresh_binding();
    let other_binding = fresh_binding();
    let owner = DeviceId::new();
    let other_owner = DeviceId::new();
    let local = request(binding, b"local materialization");
    let mut local_reader = &b"local materialization"[..];
    store
        .import_reader(owner, &local, &mut local_reader)
        .unwrap();
    let remote = request(binding, b"received from peer");
    let mut remote_reader = &b"received from peer"[..];
    store
        .import_reader(other_owner, &remote, &mut remote_reader)
        .unwrap();
    let unrelated = request(other_binding, b"other binding");
    let mut unrelated_reader = &b"other binding"[..];
    store
        .import_reader(owner, &unrelated, &mut unrelated_reader)
        .unwrap();

    store.cleanup_owner(binding, owner).unwrap();
    assert!(matches!(
        store.download(binding, &local.resource.content_hash, 0, 1),
        Err(TransferError::Unknown)
    ));
    assert_eq!(
        store
            .download(binding, &remote.resource.content_hash, 0, 1)
            .unwrap()
            .bytes,
        b"r"
    );

    store.clear_binding(binding).unwrap();
    assert!(matches!(
        store.download(binding, &remote.resource.content_hash, 0, 1),
        Err(TransferError::Unknown)
    ));
    assert_eq!(
        store
            .download(other_binding, &unrelated.resource.content_hash, 0, 1)
            .unwrap()
            .bytes,
        b"o"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_and_network_path_attacks_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
    assert!(matches!(
        FileResourceStore::new(root.join("link")),
        Err(TransferError::Invalid)
    ));
    let store = FileResourceStore::new(root).unwrap();
    let owner = DeviceId::new();
    let mut request = request(fresh_binding(), b"content");
    request.resource.relative_path = "../outside".to_owned();
    assert!(matches!(
        store.prepare(owner, &request),
        Err(TransferError::Invalid)
    ));
    assert!(matches!(
        store.download(request.binding, "../../outside", 0, 1),
        Err(TransferError::Invalid)
    ));
}
