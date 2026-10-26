//! One directory-first commit/retry/error path for full and focused LAN capture.
use super::{enrich_recovery_batch, publication};
use crate::application::ports::{ControlPlanePort, RemoteResourcePublication};
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
use rusqlite::Transaction;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn commit_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    origin: ChangeOrigin,
    operation: Uuid,
) -> Result<(), Error> {
    commit_prepared_projection(control, tx, root, origin, operation, None, None)
}

/// A verified local resource set joins its publication evidence in the same
/// directory operation before the SQLite association becomes visible.
pub(crate) fn commit_resource_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    operation: Uuid,
    location: Option<RemoteResourcePublication>,
) -> Result<(), Error> {
    commit_prepared_projection(
        control,
        tx,
        root,
        ChangeOrigin::Remote,
        operation,
        None,
        location,
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
        Some(checkpoint),
        None,
    )
}

/// SQL has only planned the result; directory persistence precedes its commit.
fn commit_prepared_projection(
    control: Option<Arc<dyn ControlPlanePort>>,
    tx: Transaction<'_>,
    root: LibraryRootId,
    origin: ChangeOrigin,
    operation: Uuid,
    publication: Option<CommitCheckpoint>,
    location: Option<RemoteResourcePublication>,
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
        if let Some(checkpoint) = publication {
            if !publication::matches_saved(&tx, manifest.library_id, checkpoint, &saved)? {
                return Err(Error::conflict(
                    "saved publication operation belongs to another ledger",
                ));
            }
        }
        saved
    } else {
        let focused = publication
            .map(|checkpoint| {
                publication::capture(durability, &tx, root, manifest.library_id, checkpoint)
            })
            .transpose()?
            .flatten();
        let mut batch = LibraryBatch::new(operation, manifest.library_id, origin, vec![]);
        if let Some(ledger) = focused {
            batch.state.push(ledger);
        } else {
            batch.records =
                super::super::super::durability_backfill::export_library_records(&tx, root)?;
            enrich_recovery_batch(&tx, root, &mut batch)?;
        }
        if let Some(location) = location {
            capture_resource_location(&mut batch, location)?;
        }
        batch
    };
    let authority = control.clone();
    let (outcome, projected) = crate::application::library_commit::commit_then_project(
        control.as_ref(),
        root,
        &batch,
        move |_| {
            // The planned rows are written but not yet visible. Catch the SQLite
            // projection up to the verified directory head inside this same
            // transaction, so the applied watermark and the visible set become
            // one atomic commit and retrying a projected page stays a no-op.
            let replay = authority
                .durability()
                .ok_or_else(|| Error::unavailable("library durability", "adapter is unavailable"))?
                .recover(root)?;
            durability_backfill::verify_covered_watermark(
                &tx,
                root,
                &replay,
                Some(authority.as_ref()),
            )?;
            durability_projection::project_commits_in_transaction(&tx, root, &replay)?;
            tx.commit().map_err(storage)
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
