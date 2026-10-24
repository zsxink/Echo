//! Portable control-plane filesystem adapter with bounded, root-constrained
//! record IO. Read access never creates or acquires a write lock.

#[cfg(test)]
use crate::application::ports::ControlPlaneInventoryPort as _;
use std::sync::{Arc, Mutex, MutexGuard};

struct AccessGuard<'a> {
    process: std::fs::File,
    // Rust drops fields in declaration order. Even if explicit unlock fails,
    // close the file before allowing another thread through the adapter gate.
    _thread: MutexGuard<'a, ()>,
}

impl Drop for AccessGuard<'_> {
    fn drop(&mut self) {
        // Release the permit while the adapter mutex is still held. Closing
        // just this handle can leave flock alive in a concurrently spawned
        // child's inherited file description until its close-on-exec runs.
        // Drop cannot report cleanup errors; closing the file remains the
        // fallback and a failed unlock can only keep future writers excluded.
        let _ = fs2::FileExt::unlock(&self.process);
    }
}

use super::portable_migration as migration;
use super::registry::RootRegistry;
use crate::application::ports::{ControlPlanePort, ManifestState};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
    CURRENT_FORMAT_VERSION,
};
use crate::error::Error;

/// Filesystem adapter for the portable `echo/` control surface.
#[derive(Clone, Debug)]
pub struct RootControlPlane {
    registry: RootRegistry,
    access: Arc<Mutex<()>>,
    pub(super) validation_cache: Arc<super::durability::ValidationCache>,
}

impl RootControlPlane {
    #[must_use]
    pub fn new(registry: RootRegistry) -> Self {
        Self {
            registry,
            access: Arc::new(Mutex::new(())),
            validation_cache: Arc::default(),
        }
    }

    fn prepare(
        &self,
        root: LibraryRootId,
        allow_incompatible: bool,
    ) -> Result<AccessGuard<'_>, Error> {
        let guard = self.prepare_write(root, allow_incompatible)?;
        if !allow_incompatible {
            super::durability::recover(&self.registry, &self.validation_cache, root)?;
        }
        Ok(guard)
    }

    // Commits perform their own recovery under this guard. Legacy writes use
    // prepare(), which also recovers before modifying any materialized files.
    fn prepare_write(
        &self,
        root: LibraryRootId,
        allow_incompatible: bool,
    ) -> Result<AccessGuard<'_>, Error> {
        let guard = self.acquire_write(root)?;
        self.prepare_locked_write(root, allow_incompatible, guard)
    }

    fn acquire_write(&self, root: LibraryRootId) -> Result<AccessGuard<'_>, Error> {
        let guard = self
            .access
            .lock()
            .map_err(|_| Error::unavailable("portable control plane", "access lock poisoned"))?;
        let lock_path = migration::path(&self.registry, root, "echo/write.lock")?;
        let parent = lock_path
            .parent()
            .ok_or_else(|| Error::conflict("missing library lock parent"))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::io("create library lock directory", e, parent))?;
        let lock_path = migration::path(&self.registry, root, "echo/write.lock")?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| Error::io("open library write lock", e, &lock_path))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|e| Error::io("acquire library write lock", e, &lock_path))?;
        // The guard must own the acquired lock before any fallible recovery or
        // validation, so every early return explicitly unlocks as well.
        Ok(AccessGuard {
            process: file,
            _thread: guard,
        })
    }

    fn prepare_locked_write<'a>(
        &self,
        root: LibraryRootId,
        allow_incompatible: bool,
        guard: AccessGuard<'a>,
    ) -> Result<AccessGuard<'a>, Error> {
        migration::recover(&self.registry, root)?;
        if !allow_incompatible {
            let manifest = self.manifest_unlocked(root)?;
            if let Some(manifest) = manifest {
                if !manifest.is_compatible() {
                    return Err(Error::UnsupportedMedia {
                        operation: "access portable library".to_owned(),
                        reason: format!("unsupported portable format {}", manifest.format_version),
                    });
                }
            }
        }
        Ok(guard)
    }

    fn prepare_read(&self, root: LibraryRootId, allow_incompatible: bool) -> Result<(), Error> {
        // A read-only mount need not contain write.lock. Recovery is an explicit
        // writer capability; readers overlay verified committed after-images.
        if !allow_incompatible
            && self
                .manifest_unlocked(root)?
                .is_some_and(|m| !m.is_compatible())
        {
            return Err(Error::UnsupportedMedia {
                operation: "read portable library".to_owned(),
                reason: "unsupported portable format".to_owned(),
            });
        }
        if migration::read(
            &self.registry,
            root,
            "echo/migrations/v1-v2-journal.json",
            1024,
        )?
        .is_some()
        {
            return Err(Error::unavailable(
                "portable migration",
                "pending format migration requires writable recovery",
            ));
        }
        Ok(())
    }

    fn manifest_unlocked(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        migration::read(
            &self.registry,
            root,
            "echo/manifest.json",
            migration::MAX_RECORD_BYTES,
        )?
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|error| Error::Storage {
                what: "manifest parse".to_owned(),
                source: Box::new(error),
            })
        })
        .transpose()
    }

    fn document_unlocked(
        &self,
        root: LibraryRootId,
        relative: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        let replay = super::durability::chain(&self.registry, &self.validation_cache, root)?;
        let records = replay
            .checkpoint
            .iter()
            .flat_map(|c| &c.records)
            .chain(replay.commits.iter().flat_map(|c| &c.batch.records));
        let mut latest = None;
        for record in records {
            if migration::record_relative(record.record())? == relative {
                latest = Some(PortableRecordDocument::V2(record.clone()));
            }
        }
        if latest.is_some() {
            return Ok(latest);
        }
        let Some(raw) =
            migration::read(&self.registry, root, relative, migration::MAX_RECORD_BYTES)?
        else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&raw).ok())
    }

    fn matching_paths(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Vec<String>, Error> {
        let uuid = uuid::Uuid::parse_str(object_uuid)
            .map_err(|_| Error::conflict("invalid portable object UUID"))?;
        if kind != RecordKind::Tombstone {
            return Ok(vec![migration::original_relative(kind, uuid, None)]);
        }
        let mut result = Vec::new();
        for relative in migration::record_files(&self.registry, root, kind)? {
            if self
                .document_unlocked(root, &relative)?
                .is_some_and(|document| {
                    document.record().kind() == kind && document.record().object_uuid() == uuid
                })
            {
                result.push(relative);
            }
        }
        if result.len() > 1 {
            return Err(Error::conflict(
                "portable tombstone UUID is ambiguous; specify its deleted kind",
            ));
        }
        Ok(result)
    }

    fn read_unlocked(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        for relative in self.matching_paths(root, kind, object_uuid)? {
            if let Some(document) = self.document_unlocked(root, &relative)? {
                if document.record().kind() == kind
                    && document.record().object_uuid().to_string() == object_uuid
                {
                    return Ok(Some(document));
                }
            }
        }
        Ok(None)
    }

    fn list_unlocked(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        let mut result = std::collections::BTreeMap::new();
        for relative in migration::record_files(&self.registry, root, kind)? {
            let Some(raw) =
                migration::read(&self.registry, root, &relative, migration::MAX_RECORD_BYTES)?
            else {
                continue;
            };
            if serde_json::from_str::<PortableRecordDocument>(&raw)
                .is_ok_and(|d| d.record().kind() == kind)
            {
                result.insert(relative, raw);
            }
        }
        let replay = super::durability::chain(&self.registry, &self.validation_cache, root)?;
        for record in replay
            .checkpoint
            .iter()
            .flat_map(|c| &c.records)
            .chain(replay.commits.iter().flat_map(|c| &c.batch.records))
            .filter(|r| r.record().kind() == kind)
        {
            let raw = String::from_utf8(migration::encode(record, migration::MAX_RECORD_BYTES)?)
                .map_err(|_| Error::conflict("portable encoding is not UTF-8"))?;
            result.insert(migration::record_relative(record.record())?, raw);
        }
        Ok(result.into_values().collect())
    }

    fn existing_record_for_write(
        &self,
        root: LibraryRootId,
        relative: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        migration::read(&self.registry, root, relative, migration::MAX_RECORD_BYTES)?
            .map(|raw| {
                serde_json::from_str::<PortableRecordDocument>(&raw).map_err(|error| {
                    Error::Storage {
                        what: "validate existing portable record before replacement".to_owned(),
                        source: Box::new(error),
                    }
                })
            })
            .transpose()
    }

    fn write_json<T: serde::Serialize>(
        &self,
        root: LibraryRootId,
        relative: &str,
        value: &T,
    ) -> Result<(), Error> {
        let bytes = migration::encode(value, migration::MAX_RECORD_BYTES)?;
        migration::write(&self.registry, root, relative, &bytes)
    }
}

impl ControlPlanePort for RootControlPlane {
    fn durability(&self) -> Option<&dyn crate::application::ports::LibraryDurabilityPort> {
        Some(self)
    }
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        let _guard = self.prepare(root, false)?;
        if !manifest.is_compatible() {
            return Err(Error::conflict("cannot write unsupported portable format"));
        }
        if let Some(current) = self.manifest_unlocked(root)? {
            if current.format_version != manifest.format_version {
                return Err(Error::conflict(
                    "portable format changes require the recoverable migration",
                ));
            }
        }
        self.write_json(root, "echo/manifest.json", manifest)
    }

    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        self.prepare_read(root, true)?;
        Ok(self
            .manifest_unlocked(root)?
            .filter(LibraryManifest::is_compatible))
    }

    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        let _guard = self.prepare(root, false)?;
        record.object_key()?;
        let relative = migration::record_relative(record)?;
        let existing = self.existing_record_for_write(root, &relative)?;
        if existing.as_ref().is_some_and(|document| {
            document.sync_metadata().is_some() && document.record() != record
        }) {
            return Err(Error::conflict(
                "changing a confirmed record requires a durable batch preserving its baseline",
            ));
        }
        if self
            .manifest_unlocked(root)?
            .is_some_and(|manifest| manifest.format_version == 2)
        {
            let relative = migration::record_relative(record)?;
            let sync = self
                .document_unlocked(root, &relative)?
                .and_then(|document| document.sync_metadata().cloned());
            self.write_json(
                root,
                &relative,
                &PortableRecordEnvelope::new(record.clone(), sync)?,
            )
        } else {
            let relative = migration::original_relative(record.kind(), record.object_uuid(), None);
            self.write_json(root, &relative, record)
        }
    }

    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        self.prepare_read(root, false)?;
        Ok(self
            .read_unlocked(root, kind, object_uuid)?
            .map(PortableRecordDocument::into_record))
    }

    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        let _guard = self.prepare(root, false)?;
        for relative in self.matching_paths(root, kind, object_uuid)? {
            migration::remove(&self.registry, root, &relative)?;
        }
        Ok(())
    }

    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        self.prepare_read(root, false)?;
        self.list_unlocked(root, kind)
    }

    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        self.prepare_read(root, true)?;
        let Some(raw) = migration::read(
            &self.registry,
            root,
            "echo/manifest.json",
            migration::MAX_RECORD_BYTES,
        )?
        else {
            return Ok(ManifestState::Absent);
        };
        let Ok(manifest) = serde_json::from_str::<LibraryManifest>(&raw) else {
            return Ok(ManifestState::Malformed);
        };
        if !manifest.is_compatible() {
            return Ok(ManifestState::Incompatible {
                format_version: manifest.format_version,
            });
        }
        Ok(ManifestState::Compatible(manifest))
    }

    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        let _guard = self.prepare(root, true)?;
        let relative = format!("echo/.probe-{}", uuid::Uuid::new_v4());
        let writable = migration::write(&self.registry, root, &relative, b"e").is_ok();
        let _ = migration::remove(&self.registry, root, &relative);
        Ok(writable)
    }

    fn write_record_envelope(
        &self,
        root: LibraryRootId,
        record: &PortableRecordEnvelope,
    ) -> Result<(), Error> {
        let _guard = self.prepare(root, false)?;
        let manifest = self.manifest_unlocked(root)?;
        let Some(_) = manifest.filter(|value| value.format_version == CURRENT_FORMAT_VERSION)
        else {
            return Err(Error::conflict(
                "portable v2 writes require a completed format upgrade",
            ));
        };
        record.validate()?;
        // The local directory identity and the host's sync library identity
        // are independent. Envelope validation checks the binding itself;
        // trusted-session authorization belongs to the sync application layer.
        let relative = migration::record_relative(record.record())?;
        self.existing_record_for_write(root, &relative)?;
        self.write_json(root, &relative, record)
    }

    fn read_record_document(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.prepare_read(root, false)?;
        self.read_unlocked(root, kind, object_uuid)
    }

    fn list_record_documents(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
    ) -> Result<Vec<PortableRecordDocument>, Error> {
        self.prepare_read(root, false)?;
        self.list_unlocked(root, kind)?
            .into_iter()
            .map(|raw| {
                serde_json::from_str(&raw).map_err(|error| Error::Storage {
                    what: "portable record parse".to_owned(),
                    source: Box::new(error),
                })
            })
            .collect()
    }

    fn read_tombstone_document(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.prepare_read(root, false)?;
        if deleted_kind == RecordKind::Tombstone {
            return Err(Error::conflict("invalid deleted kind"));
        }
        let uuid =
            uuid::Uuid::parse_str(object_uuid).map_err(|_| Error::conflict("invalid UUID"))?;
        for typed in [Some(deleted_kind), None] {
            let relative = migration::original_relative(RecordKind::Tombstone, uuid, typed);
            if let Some(document) = self.document_unlocked(root, &relative)? {
                if matches!(document.record(), PortableRecord::Tombstone(record) if record.deleted_kind == deleted_kind && record.object_uuid == uuid)
                {
                    return Ok(Some(document));
                }
            }
        }
        Ok(None)
    }

    fn delete_tombstone(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        let _guard = self.prepare(root, false)?;
        if deleted_kind == RecordKind::Tombstone {
            return Err(Error::conflict("invalid deleted kind"));
        }
        let uuid =
            uuid::Uuid::parse_str(object_uuid).map_err(|_| Error::conflict("invalid UUID"))?;
        migration::remove(
            &self.registry,
            root,
            &migration::original_relative(RecordKind::Tombstone, uuid, Some(deleted_kind)),
        )?;
        let legacy = migration::original_relative(RecordKind::Tombstone, uuid, None);
        if self.document_unlocked(root, &legacy)?.is_some_and(|document| matches!(document.record(), PortableRecord::Tombstone(record) if record.deleted_kind == deleted_kind)) {
            migration::remove(&self.registry, root, &legacy)?;
        }
        Ok(())
    }

    fn apply_portable_upgrade(
        &self,
        root: LibraryRootId,
        updates: &[PortableRecordEnvelope],
        manifest: &LibraryManifest,
    ) -> Result<(), Error> {
        let _guard = self.prepare(root, true)?;
        migration::apply(&self.registry, root, updates, manifest)
    }
}

impl crate::application::ports::LibraryDurabilityPort for RootControlPlane {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error> {
        let _guard = self.prepare(root, false)?;
        super::durability::write_asset(&self.registry, root, bytes)
    }
    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error> {
        self.prepare_read(root, false)?;
        super::durability::read_asset(&self.registry, root, hash)
    }
    fn commit(
        &self,
        root: LibraryRootId,
        batch: &crate::domain::library::durability::LibraryBatch,
    ) -> Result<crate::domain::library::durability::CommitOutcome, Error> {
        let _guard = self.prepare_write(root, false)?;
        super::durability::commit(&self.registry, &self.validation_cache, root, batch)
    }
    fn recover(
        &self,
        root: LibraryRootId,
    ) -> Result<crate::domain::library::durability::LibraryReplay, Error> {
        let _guard = self.prepare(root, true)?;
        super::durability::recover(&self.registry, &self.validation_cache, root)
    }
    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<crate::domain::library::durability::LibraryCommit>, Error> {
        self.prepare_read(root, false)?;
        super::durability::read_operation(&self.registry, &self.validation_cache, root, id)
    }
    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<crate::domain::library::durability::CommitOutcome>, Error> {
        self.prepare_read(root, false)?;
        super::durability::operation_status(&self.registry, &self.validation_cache, root, id)
    }
}

impl crate::application::ports::ControlPlaneInventoryPort for RootControlPlane {
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.prepare_read(root, false)?;
        let replay = super::durability::chain(&self.registry, &self.validation_cache, root)?;
        if replay
            .checkpoint
            .as_ref()
            .is_some_and(|c| !c.records.is_empty())
            || replay.commits.iter().any(|c| !c.batch.records.is_empty())
        {
            return Ok(true);
        }
        for &kind in RecordKind::ALL {
            if !migration::record_files(&self.registry, root, kind)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::ControlPlanePort;
    use crate::domain::ids::{LibraryRootId, Revision};
    use crate::domain::library::{
        DeviceId, HybridLogicalClock, LibraryId, LibraryRelativePath, SongRecord,
    };

    fn setup() -> (
        tempfile::TempDir,
        RootRegistry,
        LibraryRootId,
        RootControlPlane,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let plane = RootControlPlane::new(registry.clone());
        (dir, registry, root, plane)
    }

    #[test]
    fn library_durability_writer_permit_releases_lock_with_a_duplicated_handle_alive() {
        let (_dir, registry, root, plane) = setup();
        let guard = plane.prepare_write(root, true).unwrap();
        // A concurrent process spawn can temporarily inherit an open file
        // description before close-on-exec. Its lifetime is not a write permit.
        let inherited = guard.process.try_clone().unwrap();
        drop(guard);
        let next = RootControlPlane::new(registry);
        let permit = next.prepare_write(root, true).unwrap();
        drop(permit);
        drop(inherited);
    }

    #[test]
    // The competing adapter must attempt acquisition before the guard is
    // consumed by the deliberately failing preparation.
    #[allow(clippy::significant_drop_tightening)]
    fn library_durability_failed_write_preparation_unlocks_with_a_duplicated_handle_alive() {
        for relative in ["manifest.json", "migrations/v1-v2-journal.json"] {
            let (directory, registry, root, plane) = setup();
            let invalid = directory.path().join("echo").join(relative);
            std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
            std::fs::write(&invalid, "not valid JSON").unwrap();

            let guard = plane.acquire_write(root).unwrap();
            let inherited = guard.process.try_clone().unwrap();
            let competing = RootControlPlane::new(registry);
            assert!(competing.acquire_write(root).is_err());
            assert!(plane.prepare_locked_write(root, false, guard).is_err());

            // Recovery and manifest errors release the acquired permit even
            // while another handle keeps its file description alive.
            std::fs::remove_file(invalid).unwrap();
            drop(competing.prepare_write(root, false).unwrap());
            // The adapter's own thread gate must also be released on error.
            drop(plane.prepare_write(root, false).unwrap());
            drop(inherited);
        }
    }

    mod write_boundaries;

    fn sample_record(root_uuid: uuid::Uuid) -> PortableRecord {
        PortableRecord::Song(SongRecord {
            song_uuid: root_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::from_uuid(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
            ),
            hlc: HybridLogicalClock::new(1_700_000_000, 0),
            media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            added_at: 1_700_001_002_003,
            title: Some("晴天".to_owned()),
            artist: Some("周杰伦".to_owned()),
            album: None,
        })
    }

    #[test]
    fn manifest_round_trips_atomically() {
        let (_dir, _registry, root, plane) = setup();
        let manifest = LibraryManifest::new(
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            "0.1.0".to_owned(),
        );
        plane
            .write_manifest(root, &manifest)
            .expect("write manifest");
        let read = plane
            .read_manifest(root)
            .expect("read manifest")
            .expect("present");
        assert_eq!(read, manifest);
        assert!(read.is_compatible());
    }

    #[test]
    fn manifest_absent_is_none() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane.read_manifest(root).expect("read").is_none());
    }

    #[test]
    fn record_round_trips_through_shard_path() {
        let (dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        let record = sample_record(song_uuid);
        plane.write_record(root, &record).expect("write record");
        let read = plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read record")
            .expect("present");
        assert_eq!(read, record);

        // The file lives in the prefix-sharded path.
        let expected_prefix = &song_uuid.to_string()[..2];
        let echo = dir.path().join("echo");
        let record_path = echo
            .join("records")
            .join("songs")
            .join(expected_prefix)
            .join(format!("{song_uuid}.json"));
        assert!(record_path.is_file(), "record at sharded path");
    }

    #[test]
    fn record_absent_is_none() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane
            .read_record(root, RecordKind::Song, &uuid::Uuid::new_v4().to_string())
            .expect("read")
            .is_none());
    }

    #[test]
    fn delete_record_is_idempotent() {
        let (_dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write");
        plane
            .delete_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("delete");
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_none());
        // Deleting a missing record is a no-op.
        plane
            .delete_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("delete again");
    }

    #[test]
    fn tmp_files_are_ignored_as_records() {
        let (dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        // Write the real record, then place a same-dir temp file with a partial
        // JSON body (simulating an interrupted atomic write).
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write real record");
        let echo = dir.path().join("echo");
        let record_path = echo
            .join("records")
            .join("songs")
            .join(&song_uuid.to_string()[..2])
            .join(format!("{song_uuid}.json"));
        let temp = record_path.with_file_name(format!("{song_uuid}.json.tmp-deadbeef"));
        std::fs::write(&temp, "{\"partial\": true, \"unfinished").expect("write temp");
        // The reader still sees the real record; the temp is invisible.
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_some());
        // The temp never appears in listings.
        let listed = plane
            .list_records(root, RecordKind::Song)
            .expect("list records");
        assert_eq!(listed.len(), 1, "only the real record is listed");
        assert!(!listed[0].contains("unfinished"), "no partial body listed");

        // A malformed *published* record (no temp) is ignored, not surfaced.
        std::fs::write(&record_path, "{\"broken\": ").expect("corrupt record");
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_none());
        let listed = plane.list_records(root, RecordKind::Song).expect("list");
        assert_eq!(
            listed.len(),
            0,
            "malformed records are not surfaced as valid"
        );
    }

    #[test]
    fn unknown_format_manifest_is_absent() {
        let (dir, _registry, root, plane) = setup();
        let echo = dir.path().join("echo");
        std::fs::create_dir_all(&echo).expect("echo dir");
        // A future-format manifest must be treated as incompatible/absent.
        std::fs::write(
            echo.join("manifest.json"),
            format!(
                "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"0.1.0\"}}",
                uuid::Uuid::new_v4()
            ),
        )
        .expect("write future manifest");
        assert!(plane.read_manifest(root).expect("read").is_none());
    }

    #[test]
    fn control_plane_usable_detects_writable_and_readable() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane.control_plane_usable(root).expect("usable check"));
        // A usable plane writes manifest fine.
        plane
            .write_manifest(
                root,
                &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0"),
            )
            .expect("write manifest");
    }

    #[test]
    fn manifest_state_separates_absent_compatible_and_future() {
        let (dir, _registry, root, plane) = setup();
        // 1. Absent: nothing written yet.
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Absent
        );
        assert!(!plane.records_present(root).expect("records"));

        // 2. Compatible: this build's own manifest.
        let manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0");
        plane.write_manifest(root, &manifest).expect("write");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Compatible(manifest)
        );

        // 3. Future format: present but untouchable — the self-heal path must
        //    never see this as "absent".
        let echo = dir.path().join("echo");
        std::fs::write(
            echo.join("manifest.json"),
            format!(
                "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"9.9.9\"}}",
                uuid::Uuid::new_v4()
            ),
        )
        .expect("write future manifest");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Incompatible { format_version: 99 }
        );

        // 4. Unparseable: also a refusal, not a self-heal trigger.
        std::fs::write(echo.join("manifest.json"), "not json at all").expect("corrupt manifest");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Malformed
        );
    }

    #[test]
    fn records_present_ignores_temporaries_and_counts_every_kind() {
        let (dir, _registry, root, plane) = setup();
        // A kind directory holding only a temp file is *not* "records present"
        // (an interrupted write must not turn a fresh root into a heal target).
        let echo = dir.path().join("echo");
        let songs = echo.join("records").join("songs").join("ab");
        std::fs::create_dir_all(&songs).expect("shard dir");
        std::fs::write(songs.join("x.json.tmp-1"), "{}").expect("temp");
        assert!(!plane.records_present(root).expect("records"));

        // A real record of any kind makes the directory a continuation target.
        let song_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write song record");
        assert!(plane.records_present(root).expect("records"));

        let fav_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(
                root,
                &PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
                    song_uuid: fav_uuid,
                    revision: Revision::INITIAL,
                    updated_by_device_id: DeviceId::new(),
                    hlc: HybridLogicalClock::default(),
                    is_favorite: true,
                }),
            )
            .expect("write favorite record");
        assert!(plane.records_present(root).expect("records"));
    }

    #[test]
    fn list_records_sorts_and_multiples() {
        let (_dir, _registry, root, plane) = setup();
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(a))
            .expect("write a");
        plane
            .write_record(root, &sample_record(b))
            .expect("write b");
        let listed = plane.list_records(root, RecordKind::Song).expect("list");
        assert_eq!(listed.len(), 2, "two songs listed");
        // Different kinds don't collide.
        let fav = PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
            song_uuid: a,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
            is_favorite: true,
        });
        plane.write_record(root, &fav).expect("write favorite");
        assert_eq!(
            plane
                .list_records(root, RecordKind::Song)
                .expect("songs")
                .len(),
            2
        );
        assert_eq!(
            plane
                .list_records(root, RecordKind::Favorite)
                .expect("favorites")
                .len(),
            1
        );
    }
}
