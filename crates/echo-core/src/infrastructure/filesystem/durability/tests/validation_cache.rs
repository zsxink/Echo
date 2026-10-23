//! Warm-certificate adversarial reads: contextual checks must remain live.
use super::*;

// A replay through the production read API also exercises the plane-owned cache.
fn browse(plane: &RootControlPlane, root: LibraryRootId) -> Result<Vec<String>, Error> {
    plane.list_records(root, RecordKind::Playlist)
}

#[test]
fn library_durability_cache_cold_and_hot_reads_skip_only_intrinsic_validation() {
    let (_dir, registry, root, library, _) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "cached");
    commit(&registry, root, &change).unwrap();
    let plane = RootControlPlane::new(registry);
    let start = std::time::Instant::now();
    let cold = browse(&plane, root).unwrap();
    let cold_time = start.elapsed();
    assert_eq!(plane.validation_cache.counts(), (1, 0));
    let start = std::time::Instant::now();
    for _ in 0..8 {
        assert_eq!(browse(&plane, root).unwrap(), cold);
    }
    assert_eq!(plane.validation_cache.counts(), (1, 8));
    eprintln!(
        "cold {cold_time:?}; eight hot reads {:?}; intrinsic full/hits {:?}",
        start.elapsed(),
        plane.validation_cache.counts()
    );
}

#[test]
fn library_durability_cache_equal_length_tampering_and_deletion_fail_closed() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "original");
    commit(&registry, root, &change).unwrap();
    browse(&plane, root).unwrap();
    let prepared = dir
        .path()
        .join(relative(change.operation_id, "prepared.json"));
    let raw = std::fs::read_to_string(&prepared).unwrap();
    let changed = raw.replace("original", "tampered");
    assert_eq!(raw.len(), changed.len());
    std::fs::write(&prepared, &changed).unwrap();
    assert!(browse(&plane, root).is_err());
    // Errors never become certificates.
    let before = plane.validation_cache.counts();
    assert!(browse(&plane, root).is_err());
    assert_eq!(plane.validation_cache.counts().0, before.0 + 1);
    std::fs::write(&prepared, raw).unwrap();
    browse(&plane, root).unwrap();
    std::fs::remove_file(prepared).unwrap();
    assert!(browse(&plane, root).is_err());
}

#[test]
fn library_durability_cache_manifest_marker_and_chain_relationships_stay_live() {
    let (dir, registry, root, library, plane) = fixture();
    let first = batch(library, uuid::Uuid::new_v4(), "first");
    commit(&registry, root, &first).unwrap();
    browse(&plane, root).unwrap();
    let manifest_path = dir.path().join("echo/manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&LibraryManifest::new(
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            "wrong-owner",
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(browse(&plane, root).is_err());
    std::fs::write(manifest_path, original).unwrap();
    let marker_path = dir
        .path()
        .join(relative(first.operation_id, "committed.json"));
    let original = std::fs::read(&marker_path).unwrap();
    let mut marker: Marker = serde_json::from_slice(&original).unwrap();
    marker.seq += 1;
    std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(browse(&plane, root).is_err());
    std::fs::write(marker_path, original).unwrap();
    let second = batch(library, uuid::Uuid::new_v4(), "second");
    commit(&registry, root, &second).unwrap();
    browse(&plane, root).unwrap();
    // Already certified second body remains intrinsically valid, but removing
    // its predecessor must still invalidate the chain.
    std::fs::remove_file(
        dir.path()
            .join(relative(first.operation_id, "committed.json")),
    )
    .unwrap();
    assert!(browse(&plane, root).is_err());
}

#[test]
fn library_durability_cache_root_uuid_and_repointed_root_are_checked() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "root");
    commit(&registry, root, &change).unwrap();
    browse(&plane, root).unwrap();
    let other = LibraryRootId::new();
    registry.register(other, dir.path());
    browse(&plane, other).unwrap();
    assert_eq!(plane.validation_cache.counts(), (2, 0));
    let replacement = tempfile::tempdir().unwrap();
    registry.register(root, replacement.path());
    assert!(browse(&plane, root).unwrap().is_empty());
    registry.register(root, dir.path());
    assert_eq!(browse(&plane, root).unwrap().len(), 1);
}

#[test]
fn library_durability_cache_second_adapter_publication_is_visible() {
    let (_dir, registry, root, library, plane) = fixture();
    let first = batch(library, uuid::Uuid::new_v4(), "first");
    LibraryDurabilityPort::commit(&plane, root, &first).unwrap();
    assert_eq!(browse(&plane, root).unwrap().len(), 1);
    let independent = RootControlPlane::new(registry);
    let second = batch(library, uuid::Uuid::new_v4(), "second");
    LibraryDurabilityPort::commit(&independent, root, &second).unwrap();
    assert_eq!(browse(&plane, root).unwrap().len(), 2);
    assert!(
        LibraryDurabilityPort::read_operation(&plane, root, second.operation_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn library_durability_cache_coverage_and_receipt_watermark_are_rechecked() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "covered");
    commit(&registry, root, &change).unwrap();
    checkpoint::compact(
        &registry,
        &plane.validation_cache,
        root,
        &chain(&registry, root).unwrap(),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    browse(&plane, root).unwrap();
    let before = plane.validation_cache.counts();
    browse(&plane, root).unwrap();
    assert_eq!(plane.validation_cache.counts(), (before.0, before.1 + 1));
    LibraryDurabilityPort::read_operation(&plane, root, change.operation_id)
        .unwrap()
        .unwrap();
    // A cached receipt cannot establish its own durable coverage.
    let anchor = dir.path().join(ANCHOR);
    let original = std::fs::read(&anchor).unwrap();
    std::fs::remove_file(&anchor).unwrap();
    assert!(LibraryDurabilityPort::read_operation(&plane, root, change.operation_id).is_err());
    std::fs::write(&anchor, &original).unwrap();
    // Equal-length coverage mutation must force intrinsic revalidation.
    let changed = String::from_utf8(original)
        .unwrap()
        .replace("covered", "corrupt");
    std::fs::write(&anchor, changed).unwrap();
    assert!(browse(&plane, root).is_err());
}

#[test]
fn library_durability_cache_operation_identity_is_not_a_certificate() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "identity");
    commit(&registry, root, &change).unwrap();
    LibraryDurabilityPort::read_operation(&plane, root, change.operation_id)
        .unwrap()
        .unwrap();
    let new_id = uuid::Uuid::new_v4();
    std::fs::rename(
        dir.path()
            .join(format!("echo/transactions/{}", change.operation_id)),
        dir.path().join(format!("echo/transactions/{new_id}")),
    )
    .unwrap();
    assert!(browse(&plane, root).is_err());
    assert!(LibraryDurabilityPort::read_operation(&plane, root, new_id).is_err());
    std::fs::remove_file(dir.path().join(relative(new_id, "committed.json"))).unwrap();
    assert!(LibraryDurabilityPort::operation_status(&plane, root, new_id).is_err());
}

#[cfg(unix)]
#[test]
fn library_durability_cache_warm_document_never_authorizes_symlink_read() {
    let (dir, registry, root, library, plane) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "symlink");
    commit(&registry, root, &change).unwrap();
    browse(&plane, root).unwrap();
    let prepared = dir
        .path()
        .join(relative(change.operation_id, "prepared.json"));
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::copy(&prepared, outside.path()).unwrap();
    std::fs::remove_file(&prepared).unwrap();
    std::os::unix::fs::symlink(outside.path(), prepared).unwrap();
    assert!(browse(&plane, root).is_err());
}

#[test]
fn library_durability_cache_large_after_image_reads_measure_cold_and_warm_work() {
    let (_dir, registry, root, library, _) = fixture();
    let mut change = batch(library, uuid::Uuid::new_v4(), "large");
    change.state.push(DurableStateChange::new(
        StateKind::Claim,
        uuid::Uuid::new_v4(),
        serde_json::json!({"after_images": "x".repeat(1024 * 1024)}),
    ));
    commit(&registry, root, &change).unwrap();
    let plane = RootControlPlane::new(registry.clone());
    let start = std::time::Instant::now();
    for _ in 0..8 {
        browse(&plane, root).unwrap();
    }
    let warm_time = start.elapsed();
    assert_eq!(plane.validation_cache.counts(), (1, 7));
    let start = std::time::Instant::now();
    // New adapters have independent cold certificates over the same real bytes.
    for _ in 0..8 {
        let fresh = RootControlPlane::new(registry.clone());
        browse(&fresh, root).unwrap();
        assert_eq!(fresh.validation_cache.counts(), (1, 0));
    }
    eprintln!("1 MiB after-image, eight reads: warm {warm_time:?}, cold {:?}; warm intrinsic full/hits {:?}", start.elapsed(), plane.validation_cache.counts());
}
