//! Recover a stable saved operation without repeating its original create/edit.

use crate::ipc::library_recovery::LibraryRecoveryStatusDto;
use echo_core::application::library_recovery::RetryLibraryOperation;
use echo_core::domain::ids::{LibraryRootId, OperationId};
use echo_core::error::Error;

use super::AppServices;

impl AppServices {
    /// Retry only the existing operation in the expected active library.
    ///
    /// # Errors
    /// Rejects a stale root, active scan, unknown operation or failed replay.
    pub fn retry_library_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error> {
        self.require_recovery_root(root)?;
        if self.supervisor.is_scanning(root) {
            return Err(Error::conflict("library scan is running"));
        }
        let history = self.deps.history.as_ref().ok_or_else(|| {
            Error::unavailable("library recovery", "projection adapter is not configured")
        })?;
        RetryLibraryOperation::new(self.deps.control.as_ref(), history.as_ref())
            .execute(root, operation)
    }

    /// Read directory coverage and unresolved local projection evidence.
    /// # Errors
    /// Rejects a stale root or unreadable recovery evidence.
    pub fn library_recovery_status(
        &self,
        root: LibraryRootId,
    ) -> Result<LibraryRecoveryStatusDto, Error> {
        self.require_recovery_root(root)?;
        let history = self.deps.history.as_ref().ok_or_else(|| {
            Error::unavailable("library recovery", "projection adapter is not configured")
        })?;
        let durable = self.deps.control.durability().ok_or_else(|| {
            Error::unavailable("library recovery", "directory adapter is not configured")
        })?;
        let head = durable.recover(root)?.head().map_or(0, |(seq, _)| seq);
        LibraryRecoveryStatusDto::from_projection(
            root.to_string(),
            head,
            history.projection_status(root)?,
        )
    }

    /// Refresh the existing saved prefix, including a compacted coverage image.
    /// # Errors
    /// Rejects a stale root, active scan or unavailable evidence/projection.
    pub fn retry_library_projection(&self, root: LibraryRootId) -> Result<(), Error> {
        self.require_recovery_root(root)?;
        if self.supervisor.is_scanning(root) {
            return Err(Error::conflict("library scan is running"));
        }
        let history = self.deps.history.as_ref().ok_or_else(|| {
            Error::unavailable("library recovery", "projection adapter is not configured")
        })?;
        RetryLibraryOperation::new(self.deps.control.as_ref(), history.as_ref()).catch_up(root)
    }

    fn require_recovery_root(&self, root: LibraryRootId) -> Result<(), Error> {
        self.deps
            .roots
            .active_root()?
            .filter(|active| active.id() == root)
            .ok_or_else(|| Error::conflict("selected library changed"))?;
        Ok(())
    }
}
