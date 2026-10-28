//! One directory-first commit/retry/error path for full and focused LAN capture.
use super::{enrich_recovery_batch, page::RemotePage, publication, resource};
use crate::application::ports::{
    ControlPlanePort, LibraryDurabilityPort, LibraryWriteSession, RemoteResourcePublication,
};
use crate::domain::{
    ids::{LibraryRootId, OperationId},
    library::durability::{
        ChangeOrigin, CommitStatus, DurableResource, DurableStateChange, LibraryBatch, StateKey,
        StateNamespace, StateTarget,
    },
    sync::CommitCheckpoint,
};
use crate::error::Error;
use crate::infrastructure::sqlite::support::storage;
use crate::infrastructure::sqlite::{durability_backfill, durability_projection};
use rusqlite::{Connection, Transaction};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Default)]
struct PreparedCapture<'a> {
    publication: Option<CommitCheckpoint>,
    resource_publication: Option<RemoteResourcePublication>,
    remote_page: Option<RemotePage<'a>>,
}

impl PreparedCapture<'_> {
    fn build(
        self,
        durability: &dyn LibraryDurabilityPort,
        connection: &Connection,
        root: LibraryRootId,
        session: Option<&dyn LibraryWriteSession>,
        mut batch: LibraryBatch,
    ) -> Result<LibraryBatch, Error> {
        let Self {
            publication,
            resource_publication,
            remote_page,
        } = self;
        let focused = publication
            .map(|checkpoint| {
                publication::capture(durability, connection, root, batch.library_id, checkpoint)
            })
            .transpose()?
            .flatten();
        let focused_resource = match (resource_publication.as_ref(), session) {
            (Some(publication), Some(session)) => {
                resource::owner_is_covered(session.replay(), publication)?.then_some(publication)
            }
            _ => None,
        };
        let focused_page = match (remote_page.as_ref(), session) {
            (Some(page), Some(session)) => page.capture(
                connection,
                root,
                batch.library_id,
                batch.operation_id,
                session.replay(),
            )?,
            _ => None,
        };
        if let Some(ledger) = focused {
            batch.state.push(ledger);
        } else if let Some(publication) = focused_resource {
            batch
                .state
                .push(resource::capture(connection, root, publication.operation)?);
        } else if let Some(page) = focused_page {
            batch = page;
        } else {
            batch.records = durability_backfill::export_library_records(connection, root)?;
            enrich_recovery_batch(connection, root, &mut batch)?;
        }
        if let Some(location) = resource_publication.filter(|publication| publication.completed) {
            capture_resource_location(&mut batch, location)?;
        }
        Ok(batch)
    }
}

pub(crate) fn commit_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    origin: ChangeOrigin,
    operation: Uuid,
) -> Result<(), Error> {
    commit_prepared_projection(
        control,
        tx,
        root,
        origin,
        operation,
        PreparedCapture::default(),
    )
}

pub(crate) fn commit_remote_page_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    operation: Uuid,
    page: RemotePage<'_>,
) -> Result<(), Error> {
    commit_prepared_projection(
        control,
        tx,
        root,
        ChangeOrigin::Remote,
        operation,
        PreparedCapture {
            remote_page: Some(page),
            ..PreparedCapture::default()
        },
    )
}

/// Persist one complete resource-publication transition before SQL exposes its
/// claims, stage, or completed physical association.
pub(crate) fn commit_resource_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    operation: Uuid,
    publication: RemoteResourcePublication,
) -> Result<(), Error> {
    commit_prepared_projection(
        control,
        tx,
        root,
        ChangeOrigin::Remote,
        operation,
        PreparedCapture {
            resource_publication: Some(publication),
            ..PreparedCapture::default()
        },
    )
}

pub(crate) fn commit_publication_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    operation: Uuid,
    checkpoint: CommitCheckpoint,
) -> Result<(), Error> {
    commit_prepared_projection(
        control,
        tx,
        root,
        ChangeOrigin::HostConfirmation,
        operation,
        PreparedCapture {
            publication: Some(checkpoint),
            ..PreparedCapture::default()
        },
    )
}

/// SQL has only planned the result; directory persistence precedes its commit.
fn commit_prepared_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    origin: ChangeOrigin,
    operation: Uuid,
    capture: PreparedCapture<'_>,
) -> Result<(), Error> {
    let Some(control) = control else {
        // Isolated repository contracts only. Production always installs this.
        #[cfg(test)]
        return tx.commit().map_err(storage);
        #[cfg(not(test))]
        return Err(Error::unavailable(
            "library durability",
            "directory commit adapter is not configured",
        ));
    };
    let manifest = control
        .read_manifest(root)?
        .ok_or_else(|| Error::conflict("LAN write requires a portable library manifest"))?;
    let durability = control.durability().ok_or_else(|| {
        Error::unavailable(
            "library durability",
            "directory commit adapter is not configured",
        )
    })?;
    // Keep the authenticated base protected through capture and publication.
    // In particular, a focused resource transition must not race a later owner
    // after-image while deciding that no business record needs to be exported.
    let mut session = if origin == ChangeOrigin::Remote {
        durability.begin_write(root)?
    } else {
        // Host confirmations retain their immediate materialization/compaction
        // boundary before publication and independent checkpoint fencing.
        None
    };
    // Query uncertain operations before capture. A saved immutable batch always
    // wins over fresh clocks, later SQL facts, or a newly available optimization.
    // A prepared-only operation is *not* saved: its marker was never published,
    // so it still has to be planned and committed, and `commit` enforces that the
    // frozen prepared body matches this attempt.
    let saved = durability.operation_status(root, operation)?;
    let batch = if saved.is_some_and(|outcome| outcome.status != CommitStatus::NotSaved) {
        let saved = durability
            .read_operation(root, operation)?
            .ok_or_else(|| {
                Error::conflict("saved LAN operation is absent from the verified directory chain")
            })?
            .batch;
        if let Some(checkpoint) = capture.publication {
            if !publication::matches_saved(&tx, manifest.library_id, checkpoint, &saved)? {
                return Err(Error::conflict(
                    "saved publication operation belongs to another ledger",
                ));
            }
        }
        saved
    } else {
        capture.build(
            durability,
            &tx,
            root,
            session.as_deref(),
            LibraryBatch::new(operation, manifest.library_id, origin, vec![]),
        )?
    };
    batch.validate()?;
    let outcome = match session.as_mut() {
        Some(session) => session.commit(&batch)?,
        None => durability.commit(root, &batch)?,
    };
    // SQL projection may acquire other write capabilities. Its file writer
    // permit must be released before the callback, including on saved retries.
    drop(session);
    let authority = control.clone();
    let (outcome, projected) = crate::application::library_commit::project_committed(
        control.as_ref(),
        root,
        &batch,
        outcome,
        move |_| {
            project_directory_head(tx, authority.as_ref(), root, origin == ChangeOrigin::Remote)
        },
    )?;
    if projected.is_none() {
        let operation = OperationId::from_uuid(operation);
        let reason = Error::unavailable(
            "LAN projection",
            outcome
                .reason
                .unwrap_or_else(|| "saved directory operation requires recovery".to_owned()),
        );
        return Err(if outcome.status == CommitStatus::SavedPendingRecovery {
            Error::saved_pending_recovery(operation, reason)
        } else {
            Error::saved_pending_projection(operation, reason)
        });
    }
    Ok(())
}

fn project_directory_head(
    tx: Transaction<'_>,
    authority: &dyn ControlPlanePort,
    root: LibraryRootId,
    authenticated_session: bool,
) -> Result<(), Error> {
    // Planned rows are not yet visible. Remote sync holds a verified session
    // through its commit, so an authenticated journal replay avoids another
    // full tree audit while keeping the watermark and visible set in this one
    // SQL commit. Host confirmations retain strict recovery and materialization
    // before their independent publication fence advances.
    let durability = authority
        .durability()
        .ok_or_else(|| Error::unavailable("library durability", "adapter is unavailable"))?;
    let replay = if authenticated_session {
        durability.projection_replay(root)?
    } else {
        durability.recover(root)?
    };
    durability_backfill::verify_covered_watermark(&tx, root, &replay, Some(authority))?;
    durability_projection::project_commits_in_transaction(&tx, root, &replay)?;
    tx.commit().map_err(storage)
}

fn capture_resource_location(
    batch: &mut LibraryBatch,
    location: RemoteResourcePublication,
) -> Result<(), Error> {
    batch.resources = location
        .items
        .iter()
        .map(|item| DurableResource {
            relative_path: item.local_path.clone(),
            content_hash: item.target.manifest.digest.to_hex(),
            byte_length: item.target.manifest.length,
        })
        .collect();
    let raw = serde_json::to_string(&location).map_err(storage)?;
    batch.state.push(DurableStateChange::scoped(
        StateKey {
            namespace: StateNamespace::Library { library_id: batch.library_id },
            target: StateTarget::ResourceLocation { object: location.object },
        },
        serde_json::json!({"schema":"echo.library.resource-location.v1", "operation":location.operation,
            "publication":location,"original_body":raw,"completed":true}),
    )?);
    Ok(())
}
