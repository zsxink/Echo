//! Library authority and commit-checkpoint identity values.

use serde::{Deserialize, Serialize};

use super::sync::{AuthorityId, CommitSeq, Epoch, LibraryId};

/// User intent for the lifecycle of a root-scoped LAN sync session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSessionIntent {
    /// The user enabled sync and it may be restored after a normal exit.
    Enabled,
    /// The user stopped sync while retaining its pairing.
    #[default]
    Stopped,
    /// The user paused sync while retaining its pairing.
    Paused,
}

impl SyncSessionIntent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Stopped => "stopped",
            Self::Paused => "paused",
        }
    }

    /// Parse the persisted representation and fail closed on unknown values.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "stopped" => Some(Self::Stopped),
            "paused" => Some(Self::Paused),
            _ => None,
        }
    }
}

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
