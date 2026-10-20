//! Explicitly claim a user-restored current root as a fresh authority generation.
//! External backup files are never copied over a running application's database.

use echo_core::application::ports::{HostEpochRecoveryPort, HostRecoveryArchive};

use super::{
    identity_handle, AuthorityId, Epoch, HostIdentity, NativeSyncProvisioner, OsSecureStore,
    SecretBytes, SecurePublicationCheckpoint, SyncBinding, SyncRuntimeError,
};
use echo_core::domain::ids::LibraryRootId;

impl NativeSyncProvisioner {
    /// The caller stops the root's runtime and obtains explicit user intent
    /// before invoking this method. It claims the current restored catalog; it
    /// does not install an external SQLite backup into a running application.
    /// A new secure slot is durable before the DB binding changes. A crash or
    /// DB failure may leave an unreachable slot but cannot reset the old head.
    pub fn recover_restored_host(
        &self,
        root: LibraryRootId,
    ) -> Result<HostRecoveryArchive, SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let previous = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if !previous.host || previous.local_root != root {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let replacement = fresh_recovery_binding(previous.binding)?;
        let identity = new_recovery_identity(root, replacement)?;
        // Always create a separate authority/epoch handle. Neither unreadable
        // old credentials nor an old checkpoint is replaced to hide rollback.
        SecurePublicationCheckpoint::initialize_new(replacement)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let secret = SecretBytes::new(
            serde_json::to_vec(&identity).map_err(|_| SyncRuntimeError::WorkerFailed)?,
        );
        OsSecureStore
            .set(&identity_handle(replacement)?, &secret)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let receipt = self
            .database
            .rotate_host_epoch(root, previous.binding, replacement)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        *self
            .host_seen_checkpoint
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)? = None;
        self.uploaded_resources
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?
            .clear();
        Ok(receipt)
    }
}

fn fresh_recovery_binding(previous: SyncBinding) -> Result<SyncBinding, SyncRuntimeError> {
    let epoch = previous
        .epoch
        .as_u64()
        .checked_add(1)
        .filter(|epoch| i64::try_from(*epoch).is_ok())
        .ok_or(SyncRuntimeError::LibraryUnavailable)?;
    Ok(SyncBinding {
        library_id: previous.library_id,
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(epoch),
    })
}

fn new_recovery_identity(
    root: LibraryRootId,
    binding: SyncBinding,
) -> Result<HostIdentity, SyncRuntimeError> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["echo-host.local".to_owned()])
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
    Ok(HostIdentity {
        binding,
        local_root: root,
        certificate_der: cert.der().to_vec(),
        private_key_der: key_pair.serialize_der(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::domain::sync::LibraryId;

    #[test]
    fn recovery_identity_uses_new_authority_epoch_certificate_and_checkpoint_namespace() {
        let previous = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::from_u64(9),
        };
        let replacement = fresh_recovery_binding(previous).unwrap();
        assert_eq!(replacement.library_id, previous.library_id);
        assert_ne!(replacement.authority_id, previous.authority_id);
        assert_eq!(replacement.epoch.as_u64(), 10);
        assert_ne!(
            identity_handle(replacement).unwrap().as_str(),
            identity_handle(previous).unwrap().as_str()
        );
        let root = LibraryRootId::new();
        let first = new_recovery_identity(root, previous).unwrap();
        let recovered = new_recovery_identity(root, replacement).unwrap();
        assert_ne!(first.certificate_der, recovered.certificate_der);
        assert_ne!(first.private_key_der, recovered.private_key_der);
    }

    #[test]
    fn epoch_exhaustion_fails_before_initializing_any_secure_slot() {
        let previous = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::from_u64(i64::MAX as u64),
        };
        assert!(fresh_recovery_binding(previous).is_err());
    }
}
