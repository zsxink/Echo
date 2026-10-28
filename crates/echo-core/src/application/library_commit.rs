//! All user projections follow a durable library commit.
use crate::application::ports::ControlPlanePort;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, CommitStatus, LibraryBatch, LibraryCommit,
};
use crate::error::Error;

/// Commit the immutable library operation before invoking its projection.
///
/// # Errors
/// Reports validation or directory failures, preserving the operation identity
/// when committed materialization or the subsequent projection remains pending.
pub fn commit_then_project<T>(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    batch: &LibraryBatch,
    project: impl FnOnce(&LibraryCommit) -> Result<T, Error>,
) -> Result<(CommitOutcome, Option<T>), Error> {
    batch.validate()?;
    let outcome = control
        .durability()
        .ok_or_else(|| {
            Error::unavailable(
                "library durability",
                "directory commit adapter is not configured",
            )
        })?
        .commit(root, batch)?;
    project_committed(control, root, batch, outcome, project)
}

/// Commit through the directory's authenticated exclusive write session when
/// requested and available, then release its writer lock before applying the
/// SQL projection. Adapters without sessions retain standalone commit behavior.
pub fn commit_then_project_with_optional_session<T>(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    batch: &LibraryBatch,
    use_session: bool,
    project: impl FnOnce(&LibraryCommit) -> Result<T, Error>,
) -> Result<(CommitOutcome, Option<T>), Error> {
    batch.validate()?;
    let durable = control.durability().ok_or_else(|| {
        Error::unavailable(
            "library durability",
            "directory commit adapter is not configured",
        )
    })?;
    let outcome = if use_session {
        if let Some(mut session) = durable.begin_write(root)? {
            session.commit(batch)?
        } else {
            durable.commit(root, batch)?
        }
    } else {
        durable.commit(root, batch)?
    };
    project_committed(control, root, batch, outcome, project)
}

/// Finish the same projection protocol after an exclusive write session has
/// published its operation. The session must release its writer lock first.
pub(crate) fn project_committed<T>(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    batch: &LibraryBatch,
    mut outcome: CommitOutcome,
    project: impl FnOnce(&LibraryCommit) -> Result<T, Error>,
) -> Result<(CommitOutcome, Option<T>), Error> {
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
        Err(error @ Error::LibrarySavedPending { .. }) => Err(error),
        Err(error) => Err(Error::saved_pending_projection(
            crate::domain::ids::OperationId::from_uuid(outcome.operation_id),
            error,
        )),
    }
}
