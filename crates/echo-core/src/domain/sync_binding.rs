//! Library authority and commit-checkpoint identity values.

use serde::{Deserialize, Serialize};

use super::sync::{AuthorityId, CommitSeq, Epoch, LibraryId};

/// A binding is the complete scope for authority-owned ordering.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SyncBinding {
    pub library_id: LibraryId,
    pub authority_id: AuthorityId,
    pub epoch: Epoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitCheckpoint {
    pub binding: SyncBinding,
    pub sequence: CommitSeq,
    pub digest: [u8; 32],
}
