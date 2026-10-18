//! Publishes peer-uploaded resource bytes into the host library.

use std::sync::Arc;

use echo_core::application::ports::{LibraryRepository, RemoteResourceTarget};
use echo_core::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
use echo_core::domain::ids::{LibraryRootId, OperationId};
use echo_core::domain::sync::{ObjectKey, SyncBinding};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_sync::protocol::ResourceRefDto;
use echo_sync::resource_transfer::{FileResourceStore, ResourceTransferPort};

use super::remote_apply::PrivateResourceReader;
use crate::sync_runtime::SyncRuntimeError;

pub(super) fn publish_remote_resource_set(
    deps: RemoteResourcePublishDeps,
    database: &SqliteDatabase,
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
    let library = database
        .by_id(root)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        .ok_or(SyncRuntimeError::LibraryUnavailable)?;
    for (index, target) in targets.iter().enumerate() {
        let resource = ResourceRefDto::from(&target.manifest);
        let mut reader: Box<dyn std::io::Read> =
            if store.verify_complete(binding, &resource).is_ok() {
                Box::new(PrivateResourceReader {
                    store: store.clone(),
                    binding,
                    hash: target.manifest.digest.to_hex(),
                    length: target.manifest.length,
                    offset: 0,
                })
            } else {
                let candidates =
                    super::local_resource_candidates(database, &library, &target.manifest)?;
                let file = super::verified_local_resource(
                    candidates,
                    &target.manifest.digest.to_hex(),
                    target.manifest.length,
                )?
                .ok_or(SyncRuntimeError::LibraryUnavailable)?;
                Box::new(file)
            };
        publisher
            .stage(root, operation, index, reader.as_mut())
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
