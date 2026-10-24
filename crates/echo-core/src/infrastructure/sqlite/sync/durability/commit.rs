//! One directory-first commit/retry/error path for full and focused LAN capture.
use super::{enrich_recovery_batch, publication};
use crate::application::ports::ControlPlanePort;
use crate::domain::{
    ids::{LibraryRootId, OperationId},
    library::durability::{ChangeOrigin, CommitStatus, LibraryBatch},
    sync::CommitCheckpoint,
};
use crate::error::Error;
use crate::infrastructure::sqlite::support::storage;
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
    commit_prepared_projection(control, tx, root, origin, operation, None)
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
    let batch = if durability.operation_status(root, operation)?.is_some() {
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
        batch
    };
    let (outcome, projected) = crate::application::library_commit::commit_then_project(
        control.as_ref(),
        root,
        &batch,
        move |_| tx.commit().map_err(storage),
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
