//! Replaceable read-model projection, independent from legacy database rescue.
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    LibraryProjectionStatus, LibraryRecoveryRecords, LibraryReplay,
};
use crate::error::Error;

/// Apply only verified directory evidence; implementations atomically advance
/// their watermark in library order and never create new upload candidates.
pub trait LibraryProjectionPort: Send + Sync {
    fn project_library_commits(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error>;
    fn projection_status(&self, root: LibraryRootId) -> Result<LibraryProjectionStatus, Error>;
    fn read_recovery_records(&self, root: LibraryRootId) -> Result<LibraryRecoveryRecords, Error>;
}
