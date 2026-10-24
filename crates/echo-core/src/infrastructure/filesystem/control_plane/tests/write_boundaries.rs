use super::*;

#[test]
fn library_durability_legacy_manifest_refuses_new_journal_without_rewriting_evidence() {
    use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
    for version in [1, 2] {
        let (dir, _registry, root, plane) = setup();
        let mut manifest =
            LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "legacy");
        manifest.format_version = version;
        plane.write_manifest(root, &manifest).unwrap();
        let manifest_path = dir.path().join("echo/manifest.json");
        let original = std::fs::read(&manifest_path).unwrap();
        let batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![],
        );
        assert!(plane.durability().unwrap().commit(root, &batch).is_err());
        assert_eq!(std::fs::read(&manifest_path).unwrap(), original);
        assert!(!dir.path().join("echo/transactions").exists());
        assert_eq!(plane.read_manifest(root).unwrap(), Some(manifest));
    }
}

#[test]
fn library_durability_legacy_write_cannot_transfer_confirmation_to_new_value() {
    let (dir, _registry, root, plane) = setup();
    let library = uuid::Uuid::new_v4();
    plane
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(library), "test"),
        )
        .unwrap();
    let record = sample_record(uuid::Uuid::new_v4());
    let envelope: PortableRecordEnvelope = serde_json::from_value(serde_json::json!({
        "format_version": 2,
        "record": record,
        "sync": {
            "binding": {"library_id": library, "authority_id": uuid::Uuid::new_v4(), "epoch": 1},
            "object_version": 1
        }
    }))
    .unwrap();
    plane.write_record_envelope(root, &envelope).unwrap();
    let path = dir
        .path()
        .join(migration::record_relative(&record).unwrap());
    let before = std::fs::read(&path).unwrap();
    plane.write_record(root, &record).unwrap();
    let mut changed = record;
    let PortableRecord::Song(song) = &mut changed else {
        unreachable!()
    };
    song.title = Some("local edit".to_owned());
    assert!(plane.write_record(root, &changed).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn library_durability_envelope_keeps_local_and_sync_library_identities_independent() {
    let (_dir, _registry, root, plane) = setup();
    let local_library = uuid::Uuid::new_v4();
    let sync_library = uuid::Uuid::new_v4();
    plane
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(local_library), "test"),
        )
        .unwrap();
    let record = sample_record(uuid::Uuid::new_v4());
    let envelope: PortableRecordEnvelope = serde_json::from_value(serde_json::json!({
        "format_version": 2,
        "record": record,
        "sync": {
            "binding": {"library_id": sync_library, "authority_id": uuid::Uuid::new_v4(), "epoch": 1},
            "object_version": 1
        }
    })).unwrap();
    plane.write_record_envelope(root, &envelope).unwrap();
    assert_eq!(
        plane
            .read_record_document(root, record.kind(), &record.object_uuid().to_string())
            .unwrap(),
        Some(PortableRecordDocument::V2(envelope))
    );
    assert_eq!(
        plane
            .read_manifest(root)
            .unwrap()
            .unwrap()
            .library_id
            .as_uuid(),
        local_library
    );
}

#[test]
fn library_durability_envelope_write_preserves_unreadable_published_records() {
    for unknown_version in [false, true] {
        let (dir, _registry, root, plane) = setup();
        plane
            .write_manifest(
                root,
                &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
            )
            .unwrap();
        let record = sample_record(uuid::Uuid::new_v4());
        let envelope = PortableRecordEnvelope::new(record.clone(), None).unwrap();
        plane.write_record_envelope(root, &envelope).unwrap();
        let path = dir
            .path()
            .join(migration::record_relative(&record).unwrap());
        let raw = if unknown_version {
            let mut value = serde_json::to_value(&envelope).unwrap();
            value["format_version"] = serde_json::json!(999);
            serde_json::to_string(&value).unwrap()
        } else {
            "{\"broken\":".to_owned()
        };
        std::fs::write(&path, &raw).unwrap();
        assert!(plane.write_record_envelope(root, &envelope).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
    }
}

#[test]
fn library_durability_legacy_write_preserves_unreadable_published_records() {
    for v2 in [false, true] {
        for unknown_version in [false, true] {
            let (dir, _registry, root, plane) = setup();
            if v2 {
                plane
                    .write_manifest(
                        root,
                        &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
                    )
                    .unwrap();
            }
            let record = sample_record(uuid::Uuid::new_v4());
            plane.write_record(root, &record).unwrap();
            let relative = migration::record_relative(&record).unwrap();
            let path = dir.path().join(relative);
            let raw = if unknown_version {
                let mut value = serde_json::to_value(&record).unwrap();
                value["format_version"] = serde_json::json!(999);
                serde_json::to_string(&value).unwrap()
            } else {
                "{\"broken\":".to_owned()
            };
            std::fs::write(&path, &raw).unwrap();

            assert!(
                plane.write_record(root, &record).is_err(),
                "must reject overwriting unreadable evidence (v2={v2}, unknown={unknown_version})"
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
        }
    }
}
