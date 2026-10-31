//! The `ControlPlanePort` defaults are the compatibility contract for legacy
//! (v1-only) control planes: typed document reads adapt to V1, typed tombstone
//! lookup/delete refuses ambiguous identities, and v2 envelope writes fail
//! closed instead of dropping sync metadata.
//!
//! These tests drive a control plane that overrides only the required legacy
//! methods, so every assertion below runs through the trait's *default* bodies
//! rather than a filesystem/SQLite override.

use std::sync::Mutex;

use echo_core::application::ports::{ControlPlaneInventoryPort, ControlPlanePort, ManifestState};
use echo_core::domain::ids::{LibraryRootId, Revision};
use echo_core::domain::library::{
    DeviceId, HybridLogicalClock, LibraryManifest, PortableRecord, PortableRecordDocument,
    PortableRecordEnvelope, RecordKind, TombstoneRecord,
};
use echo_core::error::Error;
use uuid::Uuid;

/// A v1-only control plane: it stores raw portable JSON per kind, exactly like
/// the legacy adapters the trait defaults exist for.
#[derive(Default)]
struct LegacyPlane {
    records: Mutex<Vec<(RecordKind, String)>>,
    deletes: Mutex<Vec<(RecordKind, String)>>,
}

impl LegacyPlane {
    fn push(&self, kind: RecordKind, record: &PortableRecord) {
        self.push_raw(kind, &serde_json::to_string(record).unwrap());
    }

    fn push_raw(&self, kind: RecordKind, raw: &str) {
        self.records.lock().unwrap().push((kind, raw.to_owned()));
    }

    fn raw_of(&self, kind: RecordKind) -> Vec<String> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|(record_kind, _)| *record_kind == kind)
            .map(|(_, raw)| raw.clone())
            .collect()
    }

    fn tombstones(&self) -> Vec<String> {
        self.raw_of(RecordKind::Tombstone)
    }

    fn deletes(&self) -> Vec<(RecordKind, String)> {
        self.deletes.lock().unwrap().clone()
    }
}

impl ControlPlaneInventoryPort for LegacyPlane {
    fn records_present(&self, _root: LibraryRootId) -> Result<bool, Error> {
        Ok(!self.records.lock().unwrap().is_empty())
    }
}

impl ControlPlanePort for LegacyPlane {
    fn write_manifest(
        &self,
        _root: LibraryRootId,
        _manifest: &LibraryManifest,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn read_manifest(&self, _root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        Ok(None)
    }

    fn write_record(&self, _root: LibraryRootId, _record: &PortableRecord) -> Result<(), Error> {
        Ok(())
    }

    fn read_record(
        &self,
        _root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        Ok(self
            .raw_of(kind)
            .into_iter()
            .filter_map(|raw| serde_json::from_str::<PortableRecord>(&raw).ok())
            .find(|record| record.object_uuid().to_string() == object_uuid))
    }

    fn delete_record(
        &self,
        _root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.records.lock().unwrap().retain(|(record_kind, raw)| {
            !(*record_kind == kind
                && serde_json::from_str::<PortableRecord>(raw)
                    .is_ok_and(|record| record.object_uuid().to_string() == object_uuid))
        });
        self.deletes
            .lock()
            .unwrap()
            .push((kind, object_uuid.to_owned()));
        Ok(())
    }

    fn list_records(&self, _root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        Ok(self.raw_of(kind))
    }

    fn control_plane_usable(&self, _root: LibraryRootId) -> Result<bool, Error> {
        Ok(true)
    }

    fn manifest_state(&self, _root: LibraryRootId) -> Result<ManifestState, Error> {
        Ok(ManifestState::Absent)
    }
}

fn tombstone(object_uuid: Uuid, deleted_kind: RecordKind) -> PortableRecord {
    PortableRecord::Tombstone(TombstoneRecord {
        object_uuid,
        deleted_kind,
        revision: Revision::from_u64(3),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1000, 1),
    })
}

#[test]
fn legacy_control_plane_has_no_durability_and_fails_v2_writes_closed() {
    let plane = LegacyPlane::default();
    assert!(plane.durability().is_none());

    let envelope =
        PortableRecordEnvelope::new(tombstone(Uuid::new_v4(), RecordKind::Song), None).unwrap();
    let error = plane
        .write_record_envelope(LibraryRootId::new(), &envelope)
        .unwrap_err();
    // Failing closed matters: a silent `Ok(())` here would drop the v2 sync
    // metadata a newer device wrote.
    assert_eq!(error.code(), "unavailable");
}

#[test]
fn legacy_control_plane_adapts_v1_reads_and_decodes_both_document_versions() {
    let root = LibraryRootId::new();
    let plane = LegacyPlane::default();
    let uuid = Uuid::new_v4();
    let record = tombstone(uuid, RecordKind::Song);
    plane.push(RecordKind::Tombstone, &record);

    // Typed single read adapts the legacy record to a V1 document.
    assert_eq!(
        plane
            .read_record_document(root, RecordKind::Tombstone, &uuid.to_string())
            .unwrap(),
        Some(PortableRecordDocument::V1(record.clone()))
    );
    assert_eq!(
        plane
            .read_record_document(root, RecordKind::Tombstone, &Uuid::new_v4().to_string())
            .unwrap(),
        None
    );

    // Enumeration parses v1 payloads and v2 envelopes alike.
    plane.push_raw(
        RecordKind::Tombstone,
        &serde_json::to_string(&PortableRecordEnvelope::new(record.clone(), None).unwrap())
            .unwrap(),
    );
    let documents = plane
        .list_record_documents(root, RecordKind::Tombstone)
        .unwrap();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0], PortableRecordDocument::V1(record));
    assert!(matches!(documents[1], PortableRecordDocument::V2(_)));

    // A damaged record is a reported error, never silently skipped.
    plane.push_raw(RecordKind::Tombstone, "{not a portable record");
    let error = plane
        .list_record_documents(root, RecordKind::Tombstone)
        .unwrap_err();
    assert_eq!(error.code(), "unavailable");
}

#[test]
fn typed_tombstone_lookup_is_scoped_to_the_deleted_kind() {
    let root = LibraryRootId::new();
    let plane = LegacyPlane::default();
    let uuid = Uuid::new_v4();
    let song = tombstone(uuid, RecordKind::Song);
    plane.push(RecordKind::Tombstone, &song);
    plane.push(
        RecordKind::Tombstone,
        &tombstone(uuid, RecordKind::Playlist),
    );

    // The same object UUID may carry tombstones for two different kinds; only
    // the requested kind matches.
    let found = plane
        .read_tombstone_document(root, RecordKind::Song, &uuid.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(found.record(), &song);
    assert!(plane
        .read_tombstone_document(root, RecordKind::Playlist, &uuid.to_string())
        .unwrap()
        .is_some());
    assert!(plane
        .read_tombstone_document(root, RecordKind::Favorite, &uuid.to_string())
        .unwrap()
        .is_none());
    assert!(plane
        .read_tombstone_document(root, RecordKind::Song, &Uuid::new_v4().to_string())
        .unwrap()
        .is_none());

    // A second tombstone for the *same* kind makes the identity ambiguous.
    plane.push(RecordKind::Tombstone, &tombstone(uuid, RecordKind::Song));
    let error = plane
        .read_tombstone_document(root, RecordKind::Song, &uuid.to_string())
        .unwrap_err();
    assert_eq!(error.code(), "conflict");
    // The other kind still resolves unambiguously.
    assert!(plane
        .read_tombstone_document(root, RecordKind::Playlist, &uuid.to_string())
        .unwrap()
        .is_some());
}

#[test]
fn typed_tombstone_delete_requires_an_unambiguous_identity() {
    let root = LibraryRootId::new();
    let uuid = Uuid::new_v4();

    // A tombstone for a different deleted kind is never removed.
    let plane = LegacyPlane::default();
    plane.push(
        RecordKind::Tombstone,
        &tombstone(uuid, RecordKind::Playlist),
    );
    plane
        .delete_tombstone(root, RecordKind::Song, &uuid.to_string())
        .unwrap();
    assert!(plane.deletes().is_empty());
    assert_eq!(plane.tombstones().len(), 1);

    // The matching typed tombstone is deleted.
    plane
        .delete_tombstone(root, RecordKind::Playlist, &uuid.to_string())
        .unwrap();
    assert_eq!(
        plane.deletes(),
        vec![(RecordKind::Tombstone, uuid.to_string())]
    );
    assert!(plane.tombstones().is_empty());

    // Same UUID under two deleted kinds: a legacy adapter cannot prove which
    // tombstone the caller means, so it must refuse and delete nothing.
    let ambiguous = LegacyPlane::default();
    ambiguous.push(RecordKind::Tombstone, &tombstone(uuid, RecordKind::Song));
    ambiguous.push(
        RecordKind::Tombstone,
        &tombstone(uuid, RecordKind::Playlist),
    );
    let error = ambiguous
        .delete_tombstone(root, RecordKind::Song, &uuid.to_string())
        .unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert!(ambiguous.deletes().is_empty());
    assert_eq!(ambiguous.tombstones().len(), 2);
}
