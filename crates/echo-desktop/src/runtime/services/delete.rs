//! Echo delete / undo (task 7.3). Both obey the write gate.

use echo_core::application::delete::{DeleteSongs, RestoreDeletedOperation};
use echo_core::domain::ids::{LibraryRootId, OperationId, SongId};
use echo_core::error::Error;

impl super::AppServices {
    /// Echo delete one song; returns the undo-operation id.
    ///
    /// # Errors
    ///
    /// `Validation`/`Conflict` for an unknown or non-deletable song; `Unavailable`
    /// when writes are disabled or the root is read-only; storage errors propagate.
    pub fn delete_song(&self, root: LibraryRootId, song: SongId) -> Result<String, Error> {
        self.guard_writes()?;
        Ok(DeleteSongs::new(self.deps.as_ref())
            .delete(root, song)?
            .operation
            .to_string())
    }

    /// Undo a delete within its 10-second window; returns the restored `SongId`.
    ///
    /// # Errors
    ///
    /// `Conflict` when the undo window expired; `InvariantViolation` for a
    /// non-delete operation; `Unavailable` when writes are disabled; storage
    /// errors propagate.
    pub fn undo_delete(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<String, Error> {
        self.guard_writes()?;
        Ok(RestoreDeletedOperation::new(self.deps.as_ref())
            .restore(root, operation)?
            .to_string())
    }
}
