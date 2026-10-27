//! A reusable authority that can reject every directory commit before mutation.
use crate::application::ports::{
    ControlPlaneInventoryPort, ControlPlanePort, LibraryDurabilityPort, ManifestState,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, LibraryBatch, LibraryCommit, LibraryReplay,
};
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
};
use crate::error::Error;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Wraps the real control plane and refuses directory commits while armed, so
/// tests can hold the directory still at any point of the commit order.
pub struct RefusingAuthority {
    inner: Arc<dyn ControlPlanePort>,
    refuse_commits: AtomicBool,
}

impl RefusingAuthority {
    pub fn new(inner: Arc<dyn ControlPlanePort>) -> Self {
        Self {
            inner,
            refuse_commits: AtomicBool::new(false),
        }
    }

    pub fn set_refused(&self, refused: bool) {
        self.refuse_commits.store(refused, Ordering::Relaxed);
    }

    fn directory(&self) -> &dyn LibraryDurabilityPort {
        self.inner
            .durability()
            .expect("the wrapped control plane exposes durability")
    }
}

impl ControlPlaneInventoryPort for RefusingAuthority {
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.records_present(root)
    }
}

impl ControlPlanePort for RefusingAuthority {
    fn durability(&self) -> Option<&dyn LibraryDurabilityPort> {
        Some(self)
    }

    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        self.inner.write_manifest(root, manifest)
    }

    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        self.inner.read_manifest(root)
    }

    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        self.inner.write_record(root, record)
    }

    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        self.inner.read_record(root, kind, object_uuid)
    }

    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.inner.delete_record(root, kind, object_uuid)
    }

    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        self.inner.list_records(root, kind)
    }

    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.control_plane_usable(root)
    }

    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        self.inner.manifest_state(root)
    }

    fn write_record_envelope(
        &self,
        root: LibraryRootId,
        record: &PortableRecordEnvelope,
    ) -> Result<(), Error> {
        self.inner.write_record_envelope(root, record)
    }

    fn read_record_document(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.inner.read_record_document(root, kind, object_uuid)
    }

    fn list_record_documents(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
    ) -> Result<Vec<PortableRecordDocument>, Error> {
        self.inner.list_record_documents(root, kind)
    }

    fn read_tombstone_document(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        self.inner
            .read_tombstone_document(root, deleted_kind, object_uuid)
    }

    fn delete_tombstone(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.inner.delete_tombstone(root, deleted_kind, object_uuid)
    }
}

impl LibraryDurabilityPort for RefusingAuthority {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error> {
        self.directory().write_asset(root, bytes)
    }

    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error> {
        self.directory().read_asset(root, hash)
    }

    fn commit(&self, root: LibraryRootId, batch: &LibraryBatch) -> Result<CommitOutcome, Error> {
        if self.refuse_commits.load(Ordering::Relaxed) {
            return Err(Error::unavailable(
                "test directory",
                "simulated commit refusal",
            ));
        }
        self.directory().commit(root, batch)
    }

    fn recover(&self, root: LibraryRootId) -> Result<LibraryReplay, Error> {
        self.directory().recover(root)
    }

    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<LibraryCommit>, Error> {
        self.directory().read_operation(root, id)
    }

    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<CommitOutcome>, Error> {
        self.directory().operation_status(root, id)
    }
}
