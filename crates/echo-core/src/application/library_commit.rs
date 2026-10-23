//! All user projections follow a durable library commit.
use crate::application::ports::ControlPlanePort;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, CommitStatus, LibraryBatch, LibraryCommit,
};
use crate::error::Error;

pub fn commit_then_project<T>(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    batch: &LibraryBatch,
    project: impl FnOnce(&LibraryCommit) -> Result<T, Error>,
) -> Result<(CommitOutcome, Option<T>), Error> {
    batch.validate()?;
    let mut outcome = control
        .durability()
        .ok_or_else(|| {
            Error::unavailable(
                "library durability",
                "directory commit adapter is not configured",
            )
        })?
        .commit(root, batch)?;
    if outcome.status == CommitStatus::SavedPendingRecovery {
        return Err(Error::saved_pending_recovery(
            crate::domain::ids::OperationId::from_uuid(outcome.operation_id),
            Error::unavailable(
                "library recovery",
                "directory materialization remains pending",
            ),
        ));
    }
    let committed = control
        .durability()
        .ok_or_else(|| Error::unavailable("library durability", "adapter missing"))?
        .read_operation(root, batch.operation_id)?
        .ok_or_else(|| Error::conflict("committed operation missing from verified library"))?;
    match project(&committed) {
        Ok(value) => {
            outcome.status = CommitStatus::Complete;
            outcome.reason = None;
            Ok((outcome, Some(value)))
        }
        Err(error) => Err(Error::saved_pending_projection(
            crate::domain::ids::OperationId::from_uuid(outcome.operation_id),
            error,
        )),
    }
}
