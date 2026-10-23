use crate::domain::ids::{LibraryRootId, OperationId};
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
    CURRENT_FORMAT_VERSION,
};
use crate::error::Error;

/// The observable state of `echo/manifest.json` (design D3).
///
/// `Absent` and `Incompatible`/`Malformed` must never collapse into one
/// answer: "no manifest" is the self-heal trigger, while a manifest this build
/// cannot understand is a **refusal** — overwriting it would silently downgrade
/// a newer library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestState {
    /// No `echo/manifest.json` at all (a brand-new root, or a root whose
    /// manifest was lost while `echo/records/` survived).
    Absent,
    /// A manifest this build can read and whose format version it supports.
    Compatible(LibraryManifest),
    /// A manifest written by a newer format version. Continuation is refused
    /// and the file must be left byte-for-byte untouched.
    Incompatible { format_version: u64 },
    /// A manifest file that is present but not parseable. Same refusal as
    /// `Incompatible` — never overwritten by the self-heal path.
    Malformed,
}

impl ManifestState {
    /// The manifest when this build may act on it.
    #[must_use]
    pub const fn compatible(&self) -> Option<&LibraryManifest> {
        match self {
            Self::Compatible(manifest) => Some(manifest),
            Self::Absent | Self::Incompatible { .. } | Self::Malformed => None,
        }
    }

    /// Whether the file exists (whatever its state).
    #[must_use]
    pub const fn present(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// The format version this build writes (re-exported for adapters).
pub const SUPPORTED_FORMAT_VERSION: u64 = CURRENT_FORMAT_VERSION;

/// Cheap legacy-root evidence inventory, independent from logical mutation IO.
pub trait ControlPlaneInventoryPort: Send + Sync {
    /// Whether `echo/records/` already carries at least one object record of
    /// any kind. Cheap (directory scan, no file reads): it decides whether a
    /// manifest-less directory is a brand-new root or a self-heal candidate.
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error>;
}

pub trait ControlPlanePort: ControlPlaneInventoryPort {
    /// The optional focused durability capability. Legacy adapters fail closed.
    fn durability(&self) -> Option<&dyn super::LibraryDurabilityPort> {
        None
    }

    /// Write (create or replace) the manifest at `echo/manifest.json`.
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error>;
    /// Read the manifest. `Ok(None)` when `echo/manifest.json` is absent or
    /// not yet initialized.
    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error>;
    /// Write (create or replace) one object record. The path is derived from
    /// the record kind + UUID prefix.
    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error>;
    /// Read one object record by kind + UUID. `Ok(None)` when the record is
    /// absent or is a malformed/unsupported temporary.
    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error>;
    /// Remove one object record. Missing records are a no-op.
    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error>;
    /// Enumerate every record of one kind as their raw portable JSON (paths,
    /// not parsed content — for restore, which re-projects). Sorted by UUID.
    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error>;
    /// Whether the library's control surface is usable (the `echo/` directory
    /// can be created/updated). `Ok(false)` when the control plane is not
    /// usable — the library must not be enabled for sync/logic changes.
    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error>;
    /// The state of `echo/manifest.json`, distinguishing "no file" from "a
    /// file this build must not touch" (design D3).
    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error>;

    /// Write a v2 envelope. Implementations that do not support the recoverable
    /// format upgrade fail closed instead of silently dropping sync metadata.
    fn write_record_envelope(
        &self,
        _root: LibraryRootId,
        _record: &PortableRecordEnvelope,
    ) -> Result<(), Error> {
        Err(Error::unavailable(
            "portable v2 records",
            "this control plane does not support v2 writes",
        ))
    }

    /// Read one v1 or v2 document. The default adapts older test/control-plane
    /// implementations; filesystem implementations should override it to read
    /// v2 paths and run journal recovery first.
    fn read_record_document(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        Ok(self
            .read_record(root, kind, object_uuid)?
            .map(PortableRecordDocument::V1))
    }

    /// Enumerate typed v1/v2 documents in stable order.
    fn list_record_documents(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
    ) -> Result<Vec<PortableRecordDocument>, Error> {
        self.list_records(root, kind)?
            .into_iter()
            .map(|raw| {
                serde_json::from_str(&raw).map_err(|error| {
                    Error::unavailable_with_source(
                        "portable record",
                        "a record could not be decoded",
                        error,
                    )
                })
            })
            .collect()
    }

    /// Read a tombstone for its deleted object kind. Legacy adapters can only
    /// provide an untyped tombstone lookup, so verify the discriminator.
    fn read_tombstone_document(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecordDocument>, Error> {
        let mut matching = self
            .list_record_documents(root, RecordKind::Tombstone)?
            .into_iter()
            .filter(|document| {
                matches!(document.record(), PortableRecord::Tombstone(tombstone)
                    if tombstone.object_uuid.to_string() == object_uuid
                        && tombstone.deleted_kind == deleted_kind)
            });
        let Some(document) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err(Error::conflict("typed portable tombstone is ambiguous"));
        }
        Ok(Some(document))
    }

    /// Delete only the tombstone for the requested typed object. The default
    /// refuses if a legacy adapter cannot prove the UUID is unambiguous.
    fn delete_tombstone(
        &self,
        root: LibraryRootId,
        deleted_kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        let tombstones = self.list_record_documents(root, RecordKind::Tombstone)?;
        let same_uuid_count = tombstones
            .iter()
            .filter(|document| {
                matches!(document.record(), PortableRecord::Tombstone(tombstone)
                    if tombstone.object_uuid.to_string() == object_uuid)
            })
            .count();
        if same_uuid_count > 1 {
            return Err(Error::conflict("typed portable tombstone is ambiguous"));
        }
        let has_requested_kind = tombstones.iter().any(|document| {
            matches!(document.record(), PortableRecord::Tombstone(tombstone)
                if tombstone.object_uuid.to_string() == object_uuid
                    && tombstone.deleted_kind == deleted_kind)
        });
        if has_requested_kind {
            self.delete_record(root, RecordKind::Tombstone, object_uuid)?;
        }
        Ok(())
    }

    /// Atomically upgrade all supplied records and publish the final manifest.
    /// The filesystem adapter journals originals and publishes a temporary
    /// incompatible-format fence before replacing any record.
    fn apply_portable_upgrade(
        &self,
        _root: LibraryRootId,
        _updates: &[PortableRecordEnvelope],
        _manifest: &LibraryManifest,
    ) -> Result<(), Error> {
        Err(Error::unavailable(
            "portable migration",
            "this control plane does not support recoverable upgrades",
        ))
    }
}

// ---------------------------------------------------------------------------
// System trash
// ---------------------------------------------------------------------------

/// Outcome of moving a delete-operation's staging directory to the system
/// trash. This is the "explicitly confirmable, cross-restart" contract the
/// delete design requires (`docs/DESIGN.md` §9): the command only reports
/// success if the OS call returned success; otherwise the journal stays in
/// `TrashPending`/`TrashOutcomeUnknown`.
pub trait SystemTrashPort: Send + Sync {
    /// Move the whole `trash/<operation-id>` directory to the system trash.
    /// Its child files retain their original library basenames so the deleted
    /// song remains recognizable when the trash entry is inspected.
    ///
    /// Returns `Ok(())` ONLY when the platform call unambiguously succeeded.
    /// Any other outcome is a failure the journal must preserve.
    fn send_to_trash(&self, root: LibraryRootId, operation: OperationId) -> Result<(), Error>;
}
