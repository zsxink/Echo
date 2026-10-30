//! Directory durability capability, independent from database transactions.
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, LibraryBatch, LibraryCommit, LibraryReplay,
};
use crate::error::Error;

/// A single ordered edit whose recovery proof remains protected by the
/// directory writer lock until publication finishes.
pub trait LibraryWriteSession {
    fn replay(&self) -> &LibraryReplay;
    fn commit(&mut self, batch: &LibraryBatch) -> Result<CommitOutcome, Error>;
}

/// Content-addressed blobs owned by the library directory.
pub trait LibraryAssetPort: Send + Sync {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error>;
    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error>;
}

/// Optional exclusive write session. Adapters with an exclusive writer guard
/// can recover once and retain that proof through planning and commit; other
/// adapters use the ordinary path.
pub trait LibraryWriteSessionPort: Send + Sync {
    fn begin_write(
        &self,
        _root: LibraryRootId,
    ) -> Result<Option<Box<dyn LibraryWriteSession + '_>>, Error> {
        Ok(None)
    }
}

/// Ordered commits, operation receipts and the recovery proof they leave in
/// the directory.
pub trait LibraryDurabilityPort: LibraryAssetPort + LibraryWriteSessionPort + Send + Sync {
    fn commit(&self, root: LibraryRootId, batch: &LibraryBatch) -> Result<CommitOutcome, Error>;
    fn recover(&self, root: LibraryRootId) -> Result<LibraryReplay, Error>;
    /// Materialize a committed journal suffix for background read-model work.
    /// Implementations may skip the expensive full-tree audit here; `recover`
    /// remains the strict verification boundary.
    fn catch_up_projection(&self, root: LibraryRootId) -> Result<LibraryReplay, Error> {
        self.recover(root)
    }
    /// Read the current authenticated journal for asynchronous SQL projection
    /// without materializing the rebuildable file tree or taking the writer lock.
    fn projection_replay(&self, root: LibraryRootId) -> Result<LibraryReplay, Error> {
        self.catch_up_projection(root)
    }
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
