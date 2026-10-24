//! Recovery-only access to a consistent, typed historical projection.

use crate::domain::ids::LibraryRootId;
use crate::domain::library::PortableRecordEnvelope;
use crate::error::Error;

/// SQLite is evidence during legacy rescue and a replaceable read model after
/// rescue. The adapter keeps physical backups outside the portable library.
pub trait LibraryHistoryPort: super::LibraryProjectionPort {
    /// Capture only this root's portable facts, preserving original identities.
    fn capture_library_history(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<PortableRecordEnvelope>, Error>;
}
