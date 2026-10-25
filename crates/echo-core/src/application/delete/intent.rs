//! Directory-committed local delete intents (task 6.4).
//!
//! A delete is a *reversible local intent* until its 10-second window ends:
//! the directory records the intent, the claimed object and the identity the
//! final tombstone will carry, but never a shared final tombstone. Undo and
//! deadline finalization then compete for one terminal state through the
//! directory commit — the single place both sides can observe.
//!
//! The fact is unscoped (`DurableStateChange::new`) because a delete intent
//! is local evidence, not an authority-scoped event: it grants no send
//! authority, materializes under `echo/state/delete-intents/`, and is kept by
//! compaction so a resource still referenced by an open intent is never
//! collected.

use crate::application::library_commit::commit_then_project;
use crate::application::scan::ScanDeps;
use crate::domain::ids::{LibraryRootId, OperationId};
use crate::domain::library::durability::{
    ChangeOrigin, DurableStateChange, LibraryBatch, StateKind,
};
use crate::domain::sync::{
    DeleteIntentAction, DeleteIntentError, DeleteIntentOutcome, DeleteIntentState,
    LocalDeleteIntent, MutationId, ObjectKey,
};
use crate::error::Error;

/// Derive one deterministic commit identity per intent transition, so a retry
/// rewrites the same operation instead of appending a duplicate fact.
fn transition_id(operation: OperationId, stage: &str) -> uuid::Uuid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"echo.delete-intent");
    hasher.update(&[0]);
    hasher.update(stage.as_bytes());
    hasher.update(&[0]);
    hasher.update(operation.to_string().as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    uuid::Uuid::from_bytes(bytes)
}

fn encode(intent: &LocalDeleteIntent) -> Result<serde_json::Value, Error> {
    serde_json::to_value(intent).map_err(|error| {
        Error::unavailable_with_source("delete intent", "cannot encode the local intent", error)
    })
}

fn commit(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    stage: &str,
    intent: &LocalDeleteIntent,
) -> Result<(), Error> {
    let library = crate::application::scan::durable::library_id(deps, root)?;
    let mut batch = LibraryBatch::new(
        transition_id(operation, stage),
        library,
        ChangeOrigin::LocalEdit,
        vec![],
    );
    batch.state.push(DurableStateChange::new(
        StateKind::DeleteIntent,
        operation.as_uuid(),
        encode(intent)?,
    ));
    commit_then_project(deps.control.as_ref(), root, &batch, |_| Ok(()))?;
    Ok(())
}

const fn stage_name(state: DeleteIntentState) -> &'static str {
    match state {
        DeleteIntentState::Open => "open",
        DeleteIntentState::Cancelled => "cancelled",
        DeleteIntentState::Finalized => "finalized",
        DeleteIntentState::SupersededByRemoteTombstone => "superseded",
    }
}

/// The persisted intent of one delete operation, if the directory recorded it.
///
/// # Errors
/// Propagates replay or payload failures. An unreadable payload is an error,
/// never a silently fresh intent: a second intent would split the terminal
/// state.
fn load(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
) -> Result<Option<LocalDeleteIntent>, Error> {
    let replay = deps
        .control
        .durability()
        .ok_or_else(|| {
            Error::unavailable(
                "library durability",
                "directory commit adapter is not configured",
            )
        })?
        .recover(root)?;
    let mut latest = None;
    for state in replay.states() {
        if state.kind == StateKind::DeleteIntent && state.id == operation.as_uuid() {
            latest = Some(
                serde_json::from_value::<LocalDeleteIntent>(state.payload.clone()).map_err(
                    |error| {
                        Error::unavailable_with_source(
                            "delete intent",
                            "the committed intent payload is unreadable",
                            error,
                        )
                    },
                )?,
            );
        }
    }
    Ok(latest)
}

/// Record the reversible delete intent before any resource or database work,
/// or return the one a previous attempt already committed.
///
/// The intent identity is the operation identity: the SQLite ledger reuses it
/// so both sides name one terminal state.
///
/// # Errors
/// Rejects an unrepresentable deadline and propagates the directory commit.
pub fn begin(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    created_at_ms: u64,
) -> Result<LocalDeleteIntent, Error> {
    if let Some(existing) = load(deps, root, operation)? {
        return Ok(existing);
    }
    let mut intent = LocalDeleteIntent::begin(object, created_at_ms)
        .map_err(|_| Error::conflict("delete intent deadline is outside the supported range"))?;
    intent.intent_id = MutationId::from_uuid(operation.as_uuid());
    commit(deps, root, operation, "open", &intent)?;
    Ok(intent)
}

/// Resolve the undo action, tolerating a retry that already cancelled the
/// intent. A finalized or superseded intent refuses the undo: the terminal
/// state is unique and the final tombstone already owns the object.
///
/// # Errors
/// Propagates the directory commit and reports a conflicting terminal state.
pub fn cancel(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    now_ms: u64,
) -> Result<(), Error> {
    let mut intent = begin(deps, root, operation, object, now_ms)?;
    match intent.state {
        DeleteIntentState::Cancelled => return Ok(()),
        DeleteIntentState::Finalized => {
            return Err(Error::conflict(
                "delete intent already finalized; the object is gone",
            ))
        }
        DeleteIntentState::SupersededByRemoteTombstone => {
            return Err(Error::conflict(
                "a confirmed remote tombstone already removed the object",
            ))
        }
        DeleteIntentState::Open => {}
    }
    intent
        .resolve(DeleteIntentAction::Undo, now_ms)
        .map_err(map_error)?;
    commit(deps, root, operation, stage_name(intent.state), &intent)?;
    Ok(())
}

/// Resolve the deadline action and record the terminal state.
///
/// Returns the mutation identity the final tombstone must carry, or `None`
/// while the window is still open (a clock that went backwards never invents a
/// final state).
///
/// # Errors
/// Propagates the directory commit and reports a conflicting terminal state.
// Wired in the remaining 6.4 work: `trash::finalize_persisted_trash` must
// resolve the deadline through this before it deletes the song row.
#[allow(dead_code)]
pub fn finalize(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    now_ms: u64,
) -> Result<Option<MutationId>, Error> {
    let mut intent = begin(deps, root, operation, object, now_ms)?;
    let previous = intent.state;
    let outcome = intent
        .resolve(DeleteIntentAction::DeadlineReached, now_ms)
        .map_err(map_error)?;
    if intent.state != previous {
        commit(deps, root, operation, stage_name(intent.state), &intent)?;
    }
    Ok(match outcome {
        DeleteIntentOutcome::EmitFinalTombstone { mutation_id, .. } => Some(mutation_id),
        _ => None,
    })
}

/// Resolve a confirmed remote final tombstone for a locally intended delete.
///
/// # Errors
/// Propagates the directory commit and reports a conflicting terminal state.
// Wired in the remaining 6.4 work: the confirmed-remote-tombstone projection
// must supersede an open local intent instead of leaving it to expire.
#[allow(dead_code)]
pub fn supersede_on_remote_tombstone(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    now_ms: u64,
) -> Result<bool, Error> {
    let mut intent = begin(deps, root, operation, object, now_ms)?;
    let previous = intent.state;
    let outcome = intent
        .resolve(DeleteIntentAction::RemoteFinalTombstone, now_ms)
        .map_err(map_error)?;
    if intent.state != previous {
        commit(deps, root, operation, stage_name(intent.state), &intent)?;
    }
    Ok(matches!(
        outcome,
        DeleteIntentOutcome::RemovedByRemoteTombstone
    ))
}

fn map_error(error: DeleteIntentError) -> Error {
    match error {
        DeleteIntentError::UndoWindowExpired => Error::conflict("undo window expired"),
        DeleteIntentError::AlreadyResolved => {
            Error::conflict("delete intent already reached its terminal state")
        }
        DeleteIntentError::InvalidDeadline => {
            Error::conflict("delete intent deadline is outside the supported range")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_identities_are_stable_and_distinct() {
        let operation = OperationId::from_uuid(uuid::Uuid::new_v4());
        assert_eq!(
            transition_id(operation, "finalized"),
            transition_id(operation, "finalized")
        );
        assert_ne!(
            transition_id(operation, "finalized"),
            transition_id(operation, "cancelled")
        );
        assert_ne!(transition_id(operation, "finalized"), operation.as_uuid());
    }
}
