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

#[test]
fn corrupt_checkpoint_is_discarded_and_the_declaration_restarts_the_transfer() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let bytes = b"authoritative declaration";
    let request = request(fresh_binding(), bytes);
    let progress = store.prepare(owner, &request).unwrap();
    let partition = store.directory(request.binding).unwrap();
    let checkpoint = partition.join(format!("{}.json", progress.upload_id));
    // Simulate a torn checkpoint write: truncated JSON the loader cannot read.
    std::fs::write(&checkpoint, b"{\"owner\":").unwrap();

    let restarted = store.prepare(owner, &request).unwrap();

    assert_eq!(restarted.upload_id, progress.upload_id);
    assert!(restarted.confirmed_chunks.is_empty());
    assert!(!restarted.complete);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&checkpoint).unwrap()).is_ok(),
        "the declaration rewrites a readable checkpoint"
    );
    let mut reader = &bytes[..];
    assert!(
        store
            .import_reader(owner, &request, &mut reader)
            .unwrap()
            .complete
    );
}

#[test]
fn a_blob_that_no_longer_matches_the_declaration_is_replaced_by_retransfer() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let bytes = b"verified bytes";
    let request = request(fresh_binding(), bytes);
    let mut reader = &bytes[..];
    store.import_reader(owner, &request, &mut reader).unwrap();
    let blob = store
        .directory(request.binding)
        .unwrap()
        .join(format!("{}.blob", request.resource.content_hash));
    assert!(store
        .verify_complete(request.binding, &request.resource)
        .is_ok());
    // Damage the stored bytes while keeping the same length.
    std::fs::write(&blob, b"damaged bytes!").unwrap();

    assert!(matches!(
        store.verify_complete(request.binding, &request.resource),
        Err(TransferError::Unknown)
    ));
    assert!(!blob.exists(), "the invalid blob is removed, not retained");

    let restarted = store.prepare(owner, &request).unwrap();
    assert!(!restarted.complete, "the resource must be fetched again");
    let mut reader = &bytes[..];
    store.import_reader(owner, &request, &mut reader).unwrap();
    assert!(store
        .verify_complete(request.binding, &request.resource)
        .is_ok());
}

#[test]
fn a_checkpoint_that_describes_another_declaration_cannot_block_its_own_upload() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let binding = fresh_binding();
    let first = request(binding, b"first declaration");
    let progress = store.prepare(owner, &first).unwrap();
    // Reuse the same upload identity path with a different declared resource.
    let partition = store.directory(binding).unwrap();
    let checkpoint = partition.join(format!("{}.json", progress.upload_id));
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
    tampered["progress"]["resource"]["length"] = serde_json::json!(999);
    std::fs::write(&checkpoint, serde_json::to_vec(&tampered).unwrap()).unwrap();

    let restarted = store.prepare(owner, &first).unwrap();

    assert_eq!(restarted.resource, first.resource);
    assert!(!restarted.complete);
    let mut reader = &b"first declaration"[..];
    assert!(
        store
            .import_reader(owner, &first, &mut reader)
            .unwrap()
            .complete
    );
}

#[test]
fn cross_binding_reuse_requires_a_full_verified_declaration_match() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let old_binding = fresh_binding();
    let new_binding = SyncBinding {
        epoch: Epoch::from_u64(old_binding.epoch.as_u64() + 1),
        ..old_binding
    };
    let bytes = b"shared media bytes";
    let seeded = request(old_binding, bytes);
    let mut reader = &bytes[..];
    store.import_reader(owner, &seeded, &mut reader).unwrap();

    // The new epoch's declaration names the same bytes: the digest and length
    // are verified before the copy, so this reuse is safe.
    let matching = request(new_binding, bytes);
    assert!(store.import_cached_blob(owner, &matching).unwrap());
    assert!(store
        .verify_complete(new_binding, &matching.resource)
        .is_ok());

    // A declaration for different bytes must never be satisfied by another
    // binding's bytes, even though its blob exists under the same partition.
    let mut wrong = request(new_binding, b"different bytes!!!");
    wrong.resource.length = bytes.len() as u64;
    assert!(!store.import_cached_blob(owner, &wrong).unwrap());
    assert!(matches!(
        store.verify_complete(new_binding, &wrong.resource),
        Err(TransferError::Unknown)
    ));
}

#[test]
fn a_retired_epoch_partition_is_reclaimable_without_touching_the_live_one() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let retired = fresh_binding();
    let live = SyncBinding {
        epoch: Epoch::from_u64(retired.epoch.as_u64() + 1),
        ..retired
    };
    for (binding, bytes) in [(retired, &b"retired"[..]), (live, &b"live"[..])] {
        let request = request(binding, bytes);
        let mut reader = bytes;
        store.import_reader(owner, &request, &mut reader).unwrap();
    }

    store.clear_binding(retired).unwrap();

    assert!(matches!(
        store.download(retired, blake3::hash(b"retired").to_hex().as_ref(), 0, 1),
        Err(TransferError::Unknown)
    ));
    assert_eq!(
        store
            .download(live, blake3::hash(b"live").to_hex().as_ref(), 0, 1)
            .unwrap()
            .bytes,
        b"l"
    );
}

/// The lifecycle reference map, in one place: what a session transition may
/// reclaim, and what it must keep because a durable record still references it.
///
/// | state                                   | stop / pause / exit     | leave            | epoch change   |
/// |-----------------------------------------|-------------------------|------------------|----------------|
/// | incomplete upload (`.part` + checkpoint)| kept (resumable)        | removed          | isolated       |
/// | completed blob still declared/in-flight | kept (referenced)       | removed          | isolated       |
/// | completed blob already published        | reclaimable             | removed          | isolated       |
#[test]
fn lifecycle_reference_map_keeps_resumable_and_referenced_bytes_only() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileResourceStore::new(temporary.path().canonicalize().unwrap()).unwrap();
    let owner = DeviceId::new();
    let binding = fresh_binding();

    // A resource the authority still declares (pending mutation or snapshot).
    let referenced = request(binding, b"still referenced");
    let mut reader = &b"still referenced"[..];
    store
        .import_reader(owner, &referenced, &mut reader)
        .unwrap();
    // A resource whose mutation is confirmed and published.
    let published = request(binding, b"already published");
    let mut reader = &b"already published"[..];
    store.import_reader(owner, &published, &mut reader).unwrap();
    // An unfinished upload that must survive a stop so it can resume.
    let resumable = request(binding, b"half uploaded bytes");
    store.prepare(owner, &resumable).unwrap();

    // Stop / pause / exit: reclaim only what is complete *and* already
    // published (no longer referenced), and never touch an incomplete upload.
    store
        .cleanup_resources(
            binding,
            std::slice::from_ref(&published.resource.content_hash),
        )
        .unwrap();
    assert!(
        store.verify_complete(binding, &referenced.resource).is_ok(),
        "a still-referenced resource survives stop/pause/exit"
    );
    assert!(matches!(
        store.verify_complete(binding, &published.resource),
        Err(TransferError::Unknown)
    ));
    assert!(
        !store.prepare(owner, &resumable).unwrap().complete,
        "an incomplete upload survives for restart recovery"
    );

    // Epoch change: a new binding's partition never sees the old bytes, and the
    // old partition can be reclaimed without destroying the new one.
    let next_epoch = SyncBinding {
        epoch: Epoch::from_u64(binding.epoch.as_u64() + 1),
        ..binding
    };
    assert!(matches!(
        store.verify_complete(next_epoch, &referenced.resource),
        Err(TransferError::Unknown)
    ));

    // Leave: the retired partition goes away entirely; other partitions stay.
    store.clear_binding(binding).unwrap();
    assert!(matches!(
        store.verify_complete(binding, &referenced.resource),
        Err(TransferError::Unknown)
    ));
}
