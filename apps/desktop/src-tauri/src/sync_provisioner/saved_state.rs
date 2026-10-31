//! Consume saved facts only after the caller has verified its live credentials.
use super::{CommitCheckpoint, DeviceId, LibraryRootId, NativeSyncProvisioner, SyncRuntimeError};
use echo_core::application::library_recovery::RetryLibraryOperation;
use echo_core::application::ports::{LibraryProjectionPort, LibrarySyncRecoveryPort};
use echo_core::domain::library::durability::{RecoverySessionRole, SyncRecoveryContext};

impl NativeSyncProvisioner {
    pub(super) fn recover_saved_sync_state(
        &self,
        root: LibraryRootId,
        author: DeviceId,
        role: RecoverySessionRole,
        verified_checkpoint: CommitCheckpoint,
    ) -> Result<(), SyncRuntimeError> {
        let manifest = self
            .scan_deps
            .control
            .read_manifest(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        let retry =
            RetryLibraryOperation::new(self.scan_deps.control.as_ref(), self.database.as_ref());
        retry
            .catch_up(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let retained = self
            .database
            .read_recovery_records(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if !retained.state.iter().any(|state| {
            matches!(state.key.as_ref().map(|key| &key.namespace),
                Some(echo_core::domain::library::durability::StateNamespace::Bound { binding })
                    if *binding == verified_checkpoint.binding)
        }) {
            return Ok(());
        }
        self.database
            .retry_library_sync_state(SyncRecoveryContext {
                root,
                local_library_id: manifest.library_id,
                local_author: author,
                role,
                verified_checkpoint,
            })
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        Ok(())
    }
}
