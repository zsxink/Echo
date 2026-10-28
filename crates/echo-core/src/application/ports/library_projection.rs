//! Replaceable read-model projection, independent from legacy database rescue.
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    LibraryCommit, LibraryProjectionStatus, LibraryRecoveryRecords, LibraryReplay,
};
use crate::domain::library::{LibraryId, PortableRecordEnvelope};
use crate::error::Error;

/// A verified disposable projection. No local database path or network
/// authorization is exposed; unresolved originals remain represented in status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryProjectionValidation {
    pub library_id: LibraryId,
    pub projection: LibraryProjectionStatus,
    pub retained_records: usize,
    pub retained_state: usize,
    pub retained_resources: usize,
}

/// Apply only verified directory evidence; implementations atomically advance
/// their watermark in library order and never create new upload candidates.
pub trait LibraryProjectionPort: Send + Sync {
    /// Build and validate a fresh disposable projection without changing the
    /// active read model. Adoption must separately recheck the directory head.
    ///
    /// # Errors
    /// Rejects a mismatched library, invalid replay or incomplete candidate.
    fn validate_library_projection(
        &self,
        _root: LibraryRootId,
        _manifest_library: LibraryId,
        _replay: &LibraryReplay,
    ) -> Result<LibraryProjectionValidation, Error> {
        Err(Error::unavailable(
            "library projection validation",
            "the projection adapter does not support disposable rebuilds",
        ))
    }
    /// Validate directory evidence in isolation, then atomically replace only
    /// this root's SQLite projection. Implementations that cannot guarantee a
    /// single-transaction replacement must leave the active projection intact.
    fn rebuild_library_projection(
        &self,
        _root: LibraryRootId,
        _manifest_library: LibraryId,
        _replay: &LibraryReplay,
    ) -> Result<LibraryProjectionValidation, Error> {
        Err(Error::unavailable(
            "library projection rebuild",
            "the projection adapter does not support atomic root replacement",
        ))
    }
    /// Project supported legacy snapshots that are not covered by a verified
    /// directory replay. This is a read-only compatibility operation: it must
    /// not invent a library commit, advance a watermark or enqueue uploads.
    fn project_legacy_records(
        &self,
        _root: LibraryRootId,
        _records: &[PortableRecordEnvelope],
    ) -> Result<(), Error> {
        Err(Error::unavailable(
            "legacy library projection",
            "the projection adapter does not support legacy snapshots",
        ))
    }
    fn project_library_commits(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error>;
    /// Project a replay returned by the directory durability adapter, using
    /// the current SQLite watermark to apply only its authenticated suffix.
    /// Implementations without a suffix path retain the full projection path.
    ///
    /// This operation also retries retained dependencies after applying the
    /// suffix, since a newly imported song can satisfy an older playlist item.
    fn project_verified_library_suffix(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error> {
        self.project_library_commits(root, replay)
    }
    /// Apply one directory-verified commit when it directly follows the
    /// current SQLite watermark. Returns `false` when a catch-up replay is
    /// needed first.
    fn project_verified_library_commit(
        &self,
        _root: LibraryRootId,
        _commit: &LibraryCommit,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    fn projection_status(&self, root: LibraryRootId) -> Result<LibraryProjectionStatus, Error>;
    fn read_recovery_records(&self, root: LibraryRootId) -> Result<LibraryRecoveryRecords, Error>;
}
