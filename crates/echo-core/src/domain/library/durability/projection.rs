//! Typed recovery evidence and a path-free status for replaceable projections.
use super::{DurableResource, DurableStateChange, StateKind};
use crate::domain::{library::PortableRecordEnvelope, sync::ObjectKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionTarget {
    Record { object: ObjectKey },
    State { state_kind: StateKind, id: uuid::Uuid },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionUnresolvedReason {
    MissingSong,
    MissingPlaylist,
    IdentityConflict,
    TombstonedDependency,
    /// Original state remains retained; a specialized recovery use case must
    /// validate its semantics and trust before installing a live projection.
    RequiresStateRecovery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnresolvedProjectionRecord {
    pub target: ProjectionTarget,
    pub reason: ProjectionUnresolvedReason,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LibraryProjectionStatus {
    pub applied_library_commit_seq: u64,
    pub applied_head_digest: Option<String>,
    pub unresolved: Vec<UnresolvedProjectionRecord>,
}

/// Complete retained typed after-images, including relations awaiting parents.
/// These values are evidence, never authorization to activate a network session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryRecoveryRecords {
    pub records: Vec<PortableRecordEnvelope>,
    pub state: Vec<DurableStateChange>,
    pub resources: Vec<DurableResource>,
}
