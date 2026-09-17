use super::*;

pub trait ControlPlanePort: Send + Sync {
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
    ///
    /// Returns `Ok(())` ONLY when the platform call unambiguously succeeded.
    /// Any other outcome is a failure the journal must preserve.
    fn send_to_trash(&self, root: LibraryRootId, operation: OperationId) -> Result<(), Error>;
}
