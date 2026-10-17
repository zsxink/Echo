//! Publishes peer-uploaded resource bytes into the host library.

use std::sync::Arc;

use echo_core::application::ports::RemoteResourceTarget;
use echo_core::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
use echo_core::domain::ids::{LibraryRootId, OperationId};
use echo_core::domain::sync::{ObjectKey, SyncBinding};
use echo_sync::protocol::ResourceRefDto;
use echo_sync::resource_transfer::{FileResourceStore, ResourceTransferPort};

use super::remote_apply::PrivateResourceReader;
use crate::sync_runtime::SyncRuntimeError;

pub(super) fn publish_remote_resource_set(
    deps: RemoteResourcePublishDeps,
    store: &Arc<FileResourceStore>,
    binding: SyncBinding,
    root: LibraryRootId,
    object: ObjectKey,
    targets: &[RemoteResourceTarget],
) -> Result<(), SyncRuntimeError> {
    let mut existing = None;
    for operation in deps
        .journal
        .incomplete(root)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
    {
        let publication = deps
            .journal
            .load(root, operation)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if publication.object == object
            && publication.items.len() == targets.len()
            && publication
                .items
                .iter()
                .zip(targets)
                .all(|(item, target)| item.target.manifest == target.manifest)
        {
            existing = Some(operation);
            break;
        }
    }
    let publisher = PublishRemoteResources::new(deps);
    let operation = existing.unwrap_or_else(OperationId::new);
    if existing.is_none() {
        publisher
            .prepare(root, operation, object, targets.to_vec())
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    }
    for (index, target) in targets.iter().enumerate() {
        store
            .verify_complete(binding, &ResourceRefDto::from(&target.manifest))
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let mut reader = PrivateResourceReader {
            store: store.clone(),
            binding,
            hash: target.manifest.digest.to_hex(),
            length: target.manifest.length,
            offset: 0,
        };
        publisher
            .stage(root, operation, index, &mut reader)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    }
    let outcome = publisher
        .resume(root, operation)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !outcome.media_ready {
        return Err(SyncRuntimeError::ResourcePublicationPending);
    }
    Ok(())
}
