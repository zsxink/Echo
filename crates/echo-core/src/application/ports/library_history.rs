//! Recovery-only access to a consistent, typed historical projection.

use crate::domain::ids::LibraryRootId;
use crate::domain::library::{durability::LibraryReplay, PortableRecordEnvelope};
use crate::error::Error;

/// SQLite is evidence during legacy rescue and a replaceable read model after
/// rescue. The adapter keeps physical backups outside the portable library.
pub trait LibraryHistoryPort: Send + Sync {
    /// Capture only this root's portable facts, preserving original identities.
    fn capture_library_history(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<PortableRecordEnvelope>, Error>;
    /// Replay the ordered library chain atomically, without creating uploads.
    fn project_library_commits(
        &self,
        root: LibraryRootId,
        commits: &LibraryReplay,
    ) -> Result<(), Error>;
    /// Read the applied directory watermark and typed retained recovery gaps.
    fn projection_status(
        &self,
        root: LibraryRootId,
    ) -> Result<crate::domain::library::durability::LibraryProjectionStatus, Error>;
    /// Read complete after-images for dependency retry and recovery inspection.
    fn read_recovery_records(
        &self,
        root: LibraryRootId,
    ) -> Result<crate::domain::library::durability::LibraryRecoveryRecords, Error>;
}
