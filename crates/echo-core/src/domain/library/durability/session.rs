//! Explicit live-session evidence supplied by the owning synchronization use case.
use crate::domain::{
    ids::LibraryRootId,
    library::LibraryId,
    sync::{CommitCheckpoint, DeviceId},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoverySessionRole {
    Host,
    Replica,
}

/// This value is deliberately not serializable: portable binding metadata cannot
/// grant this capability. The caller must already have validated the live session
/// credentials and its independent checkpoint. SQL rechecks selection and author
/// in the same transaction that projects facts; this never starts a session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncRecoveryContext {
    pub root: LibraryRootId,
    pub local_library_id: LibraryId,
    pub local_author: DeviceId,
    pub role: RecoverySessionRole,
    pub verified_checkpoint: CommitCheckpoint,
}
