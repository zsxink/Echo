//! Declared user originals and managed versions are verified before evidence.
use super::*;

/// A sync-managed immutable version: `echo/media/<kind>/<owner>/<declaration>/<path>`.
const MANAGED: &str = "echo/media/song/aaaaaaaaaaaa/declaration/track.flac";

fn managed_bytes() -> &'static [u8] {
    b"managed version bytes that Echo never rewrites"
}

fn write_managed(directory: &std::path::Path) {
    let path = directory.join(MANAGED);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, managed_bytes()).unwrap();
}

/// A batch declaring the managed version at its actual library location.
fn declaring(library: LibraryId) -> LibraryBatch {
    let mut change = batch(library, uuid::Uuid::new_v4(), "declares a managed version");
    change.resources.push(DurableResource {
        relative_path: MANAGED.to_owned(),
        content_hash: blake3::hash(managed_bytes()).to_hex().to_string(),
        byte_length: u64::try_from(managed_bytes().len()).unwrap(),
    });
    change
}

fn assert_conflict(error: &Error, reason: &str) {
    match error {
        Error::Conflict { what, .. } => assert!(what.contains(reason), "{reason}: {what}"),
        other => panic!("{reason}: unexpected refusal {other:?}"),
    }
}

#[test]
fn library_durability_unverifiable_managed_version_is_refused_before_any_evidence() {
    let mut same_length = managed_bytes().to_vec();
    // Same length, different bytes: only the content hash can tell them apart.
    same_length[0] ^= 0xff;
    for (bytes, reason) in [
        (b"truncated".to_vec(), "durable resource length mismatch"),
        (same_length, "durable resource hash mismatch"),
    ] {
        let (directory, registry, root, library, _) = fixture();
        write_managed(directory.path());
        std::fs::write(directory.path().join(MANAGED), &bytes).unwrap();
        let change = declaring(library);
        let error = commit(&registry, root, &change).unwrap_err();
        assert_conflict(&error, reason);
        // The refusal happens before any durable evidence exists, so a caller
        // cannot mistake the operation for saved.
        assert!(!marker_present(directory.path(), change.operation_id));
        assert!(
            !directory
                .path()
                .join(relative(change.operation_id, "prepared.json"))
                .exists()
        );
        assert!(
            operation_status(&registry, root, change.operation_id)
                .unwrap()
                .is_none()
        );
        assert!(chain(&registry, root).unwrap().commits.is_empty());
        // The mismatching bytes stay exactly as found: Echo reports a changed
        // file, it never rewrites or "repairs" a media file to match a
        // declaration.
        assert_eq!(
            std::fs::read(directory.path().join(MANAGED)).unwrap(),
            bytes
        );
    }
}

#[test]
fn library_durability_missing_managed_version_is_never_synthesized() {
    let (directory, registry, root, library, _) = fixture();
    let change = declaring(library);
    let error = commit(&registry, root, &change).unwrap_err();
    assert_eq!(error.code(), "io", "{error:?}");
    assert!(!marker_present(directory.path(), change.operation_id));
    assert!(chain(&registry, root).unwrap().commits.is_empty());
    assert!(!directory.path().join(MANAGED).exists());
}

#[test]
fn library_durability_tampered_user_original_is_refused_and_kept_for_diagnosis() {
    let (directory, registry, root, library, plane) = fixture();
    let bytes = b"user cover original";
    let hash = LibraryDurabilityPort::write_asset(&plane, root, bytes).unwrap();
    let relative_asset = format!("echo/assets/{hash}");
    let mut tampered = bytes.to_vec();
    tampered[0] ^= 0x01;
    std::fs::write(directory.path().join(&relative_asset), &tampered).unwrap();
    let mut change = batch(library, uuid::Uuid::new_v4(), "declares a corrupt original");
    change.resources.push(DurableResource {
        relative_path: relative_asset.clone(),
        content_hash: hash.clone(),
        byte_length: u64::try_from(bytes.len()).unwrap(),
    });
    let error = commit(&registry, root, &change).unwrap_err();
    assert_conflict(&error, "durable resource hash mismatch");
    assert!(!marker_present(directory.path(), change.operation_id));
    assert!(chain(&registry, root).unwrap().commits.is_empty());
    // Readers see the same corruption, and the bytes stay put as evidence.
    let read = LibraryDurabilityPort::read_asset(&plane, root, &hash).unwrap_err();
    assert_conflict(&read, "immutable asset is corrupt");
    assert_eq!(
        std::fs::read(directory.path().join(&relative_asset)).unwrap(),
        tampered
    );
    // A malformed address is rejected before any path is resolved.
    assert_conflict(
        &LibraryDurabilityPort::read_asset(&plane, root, "not-a-digest").unwrap_err(),
        "invalid asset hash",
    );
}

#[test]
fn library_durability_resources_must_name_media_or_an_exact_original() {
    let digest = "c".repeat(64);
    for relative in [
        "echo/state/materialized-checkpoint.json",
        "echo/records/songs/aa/aaaaaaaa.json",
        "echo/media",
    ] {
        let (directory, registry, root, library, _) = fixture();
        let mut change = batch(library, uuid::Uuid::new_v4(), "control-plane resource");
        change.resources.push(DurableResource {
            relative_path: relative.to_owned(),
            content_hash: digest.clone(),
            byte_length: 1,
        });
        let error = commit(&registry, root, &change).unwrap_err();
        assert_conflict(&error, "resource is not media or an immutable asset");
        assert!(!marker_present(directory.path(), change.operation_id));
        assert!(chain(&registry, root).unwrap().commits.is_empty());
    }
    let asset = format!("echo/assets/{digest}");
    for (relative, hash) in [
        // An original is addressed by its own content hash, not an arbitrary one.
        (asset.clone(), "d".repeat(64)),
        // Digests are canonical lowercase hex.
        (asset, digest.to_uppercase()),
        // Media versions need a real digest too.
        (
            "echo/media/song/x/track.flac".to_owned(),
            "short".to_owned(),
        ),
    ] {
        let (directory, registry, root, library, _) = fixture();
        let mut change = batch(library, uuid::Uuid::new_v4(), "unaddressed resource");
        change.resources.push(DurableResource {
            relative_path: relative,
            content_hash: hash,
            byte_length: 1,
        });
        let error = commit(&registry, root, &change).unwrap_err();
        assert_conflict(&error, "resource requires a BLAKE3 digest");
        assert!(!marker_present(directory.path(), change.operation_id));
        assert!(chain(&registry, root).unwrap().commits.is_empty());
    }
}

#[test]
fn library_durability_managed_version_declaration_survives_checkpoint_and_fresh_rebuild() {
    let (directory, registry, root, library, plane) = fixture();
    write_managed(directory.path());
    let asset_bytes = b"user cover original";
    let asset = LibraryDurabilityPort::write_asset(&plane, root, asset_bytes).unwrap();
    let asset_path = format!("echo/assets/{asset}");
    let mut change = declaring(library);
    change.resources.push(DurableResource {
        relative_path: asset_path.clone(),
        content_hash: asset,
        byte_length: u64::try_from(asset_bytes.len()).unwrap(),
    });
    commit(&registry, root, &change).unwrap();
    // The declaration is frozen with the operation, not only in SQLite.
    assert_eq!(
        read_operation(&registry, root, change.operation_id)
            .unwrap()
            .unwrap()
            .batch
            .resources,
        change.resources
    );
    // Force compaction: the journal is replaced by the coverage image.
    let replay = recover(&registry, root).unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &replay,
        &mut |_, _| Ok(()),
    )
    .unwrap();
    let collected = recover(&registry, root).unwrap();
    assert!(collected.commits.is_empty());
    let image = collected.checkpoint.as_ref().unwrap();
    for resource in &change.resources {
        assert!(
            image.resources.contains(resource),
            "coverage image lost {resource:?}"
        );
    }
    // The actual bytes are still at the locations the declarations name.
    assert_eq!(
        std::fs::read(directory.path().join(MANAGED)).unwrap(),
        managed_bytes()
    );
    assert_eq!(
        std::fs::read(directory.path().join(&asset_path)).unwrap(),
        asset_bytes
    );
    // A database built only from the directory knows every actual location,
    // hash and length, not just the object references.
    let database_dir = tempfile::tempdir().unwrap();
    let fresh =
        crate::infrastructure::sqlite::SqliteDatabase::open(database_dir.path().join("fresh.db"))
            .unwrap();
    LibraryRepository::upsert(
        &fresh,
        &LibraryRoot::new(root, directory.path().to_owned(), true, true),
    )
    .unwrap();
    fresh.configure_library_control(Arc::new(plane));
    fresh.project_library_commits(root, &collected).unwrap();
    let rebuilt = fresh.read_recovery_records(root).unwrap();
    let mut expected = change.resources.clone();
    expected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    assert_eq!(rebuilt.resources, expected);
}
