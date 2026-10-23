//! Directory durability capability, independent from database transactions.
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, LibraryBatch, LibraryCommit, LibraryReplay,
};
use crate::error::Error;
pub trait LibraryDurabilityPort: Send + Sync {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error>;
    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error>;
    fn commit(&self, root: LibraryRootId, batch: &LibraryBatch) -> Result<CommitOutcome, Error>;
    fn recover(&self, root: LibraryRootId) -> Result<LibraryReplay, Error>;
    /// Read one immutable operation by identity without enumerating history.
    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<LibraryCommit>, Error>;
    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<CommitOutcome>, Error>;
}
