//! Explicit authority recovery retains old candidates instead of replaying them.

use crate::domain::ids::LibraryRootId;
use crate::domain::sync::SyncBinding;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable recovery receipt. It contains no credentials or local file paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostRecoveryArchive {
    pub recovery_id: Uuid,
    pub local_root: LibraryRootId,
    pub previous_binding: SyncBinding,
    pub replacement_binding: SyncBinding,
    pub pending_count: u64,
    pub conflict_count: u64,
}

/// Preserved values require an explicit user decision before becoming new edits.
/// Original mutation rows (including their resource declarations) remain in the
/// archive; this view never reuses their old identity in the replacement epoch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostRecoveryCandidate {
    pub identity: String,
    pub object_type: String,
    pub object_uuid: Uuid,
    pub payload_json: String,
    pub competing_payload_json: Option<String>,
    pub resource_refs_json: String,
}

/// Confirmed identity facts recovered from v2 portable records when the host
/// ledger itself was lost. No version or cursor from them is reused.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveredAuthorityFacts {
    pub tombstones: Vec<crate::domain::sync::ObjectKey>,
    pub aliases: Vec<crate::domain::library::PortableAlias>,
}

/// Call only after stopping the root's network workers. The platform must
/// initialize a distinct independent genesis checkpoint and credentials before
/// committing the replacement binding. All old rows are retained atomically.
pub trait HostEpochRecoveryPort: Send + Sync {
    fn rotate_host_epoch(
        &self,
        root: LibraryRootId,
        expected: SyncBinding,
        replacement: SyncBinding,
    ) -> Result<HostRecoveryArchive, Error>;

    /// Explicit recovery from portable authority evidence only. Refuses any
    /// bound live context, so it cannot overwrite a complete running library.
    fn recover_unbound_host(
        &self,
        root: LibraryRootId,
        previous: SyncBinding,
        replacement: SyncBinding,
        facts: RecoveredAuthorityFacts,
    ) -> Result<HostRecoveryArchive, Error>;

    fn host_recovery_archives(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<HostRecoveryArchive>, Error>;

    fn host_recovery_candidates(
        &self,
        recovery_id: Uuid,
    ) -> Result<Vec<HostRecoveryCandidate>, Error>;
}
