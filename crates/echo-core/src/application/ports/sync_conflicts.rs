//! Explicit recovery of ordinary values from the host's durable conflict history.

use crate::domain::ids::LibraryRootId;
use crate::domain::sync::SyncBinding;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Only ordinary user fields are exposed. Structural identities, paths,
/// resources, membership and counters cannot be replayed through this API.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflictView {
    pub conflict_id: Uuid,
    pub object_type: String,
    pub object_uuid: Uuid,
    pub replaced_version: u64,
    pub accepted_version: u64,
    pub replaced_fields: serde_json::Value,
    pub accepted_fields: serde_json::Value,
    pub recovery: SyncConflictRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncConflictRecovery {
    Available,
    PendingEdit,
    FinalDeleted,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncConflictResolution {
    Recovered,
    Dismissed,
    AlreadyResolved,
    FinalDeleted,
    PendingEdit,
    Unsupported,
    Unavailable,
}

/// Host-local access only. The root and expected authority are checked in the
/// same transaction as recovery, so stale UI requests cannot cross roots or
/// epochs. Recovering queues a fresh local mutation and resolves the conflict
/// atomically; it never reuses the historical event or revives an identity.
pub trait HostSyncConflictPort: Send + Sync {
    fn host_sync_conflicts(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        limit: usize,
    ) -> Result<Vec<SyncConflictView>, Error>;

    fn resolve_host_sync_conflict(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        conflict: Uuid,
        recover_replaced: bool,
    ) -> Result<SyncConflictResolution, Error>;
}
