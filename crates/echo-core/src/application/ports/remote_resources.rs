//! Durable publication intent for downloaded library resources.

use crate::domain::ids::{LibraryRootId, OperationId};
use crate::domain::sync::{ObjectKey, ResourceDigest, SharedResourceManifest};
use crate::error::Error;
use serde::{Deserialize, Serialize};

/// A host declaration and the hash of the local file it may replace. `None`
/// requires an absent destination (or identical, fully verified content).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteResourceTarget {
    pub manifest: SharedResourceManifest,
    pub expected_old_digest: Option<ResourceDigest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RemoteResourceStage {
    Planned,
    Validated,
    OldIsolated,
    Published,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteResourceItem {
    pub target: RemoteResourceTarget,
    pub stage: RemoteResourceStage,
    /// Root-relative, marker-verified Echo staging location.
    pub staging_path: String,
    /// Immutable local version path. Shared manifests retain logical paths.
    pub local_path: String,
    /// Retained previous local version, verified before association changes.
    pub old_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteResourcePublication {
    pub root: LibraryRootId,
    pub operation: OperationId,
    pub object: ObjectKey,
    pub items: Vec<RemoteResourceItem>,
    pub completed: bool,
}

/// These writes do not create local mutations or change the object cursor.
/// Preparing reserves destination claims; finalizing verifies the declaration
/// still belongs to the current object and atomically associates all files.
pub trait RemoteResourceJournalPort: Send + Sync {
    fn prepare(&self, publication: &RemoteResourcePublication) -> Result<(), Error>;
    fn load(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<RemoteResourcePublication, Error>;
    fn save(&self, publication: &RemoteResourcePublication) -> Result<(), Error>;
    fn finish(&self, publication: &RemoteResourcePublication) -> Result<(), Error>;
    fn incomplete(&self, root: LibraryRootId) -> Result<Vec<OperationId>, Error>;
}
