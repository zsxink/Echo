//! Durable locations of immutable resources, independent of catalog rows.

use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::domain::sync::ResourceDigest;
use crate::error::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedResource {
    pub digest: ResourceDigest,
    pub length: u64,
    pub path: RelativeMediaPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteDeleteIsolation {
    pub operation: OperationId,
    pub source: RelativeMediaPath,
    pub digest: ResourceDigest,
}

/// No resource reclamation is permitted unless reconstruction succeeds.
pub trait ResourceRetentionPort: Send + Sync {
    fn rebuild_resource_holds(&self, root: LibraryRootId) -> Result<(), Error>;
    fn record_retained_resource(
        &self,
        root: LibraryRootId,
        resource: RetainedResource,
    ) -> Result<(), Error>;
    fn retained_resource(
        &self,
        root: LibraryRootId,
        digest: ResourceDigest,
    ) -> Result<Option<RetainedResource>, Error>;
    fn remote_delete_isolations(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<RemoteDeleteIsolation>, Error>;
    fn finish_remote_delete_isolation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error>;
    fn remote_publication_matches(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        digest: &str,
    ) -> Result<bool, Error>;
}
