//! A live session may consume retained facts without provisioning trust.
use crate::domain::library::durability::{LibraryProjectionStatus, SyncRecoveryContext};
use crate::error::Error;

pub trait LibrarySyncRecoveryPort: Send + Sync {
    /// Retry facts only under an already validated current session. Missing or
    /// ambiguous dependencies remain in the typed projection report.
    fn retry_library_sync_state(
        &self,
        context: SyncRecoveryContext,
    ) -> Result<LibraryProjectionStatus, Error>;
}
