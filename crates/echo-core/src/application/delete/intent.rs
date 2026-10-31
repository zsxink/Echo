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
use std::collections::HashMap;

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
        if existing.object != object {
            return Err(Error::conflict(
                "delete operation identity refers to a different object",
            ));
        }
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
pub fn finalize(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    now_ms: u64,
    tombstone: crate::domain::library::PortableRecordEnvelope,
) -> Result<Option<MutationId>, Error> {
    let mut intent = match load(deps, root, operation)? {
        Some(intent) => intent,
        None => begin(deps, root, operation, object, now_ms)?,
    };
    if intent.object != object {
        return Err(Error::conflict(
            "delete operation identity refers to a different object",
        ));
    }
    let outcome = intent
        .resolve(DeleteIntentAction::DeadlineReached, now_ms)
        .map_err(map_error)?;
    let mutation_id = match outcome {
        DeleteIntentOutcome::Pending => return Ok(None),
        DeleteIntentOutcome::EmitFinalTombstone {
            mutation_id,
            object: resolved,
        } if resolved == object => mutation_id,
        DeleteIntentOutcome::RemovedByRemoteTombstone => {
            return Err(Error::conflict(
                "a confirmed remote tombstone already removed the object",
            ));
        }
        _ => return Err(Error::conflict("delete intent cannot be finalized")),
    };
    let expected_uuid = match object {
        ObjectKey::Song(song) => song.as_uuid(),
        _ => {
            return Err(Error::conflict(
                "delete finalization requires a song object",
            ))
        }
    };
    if !matches!(tombstone.record(), crate::domain::library::PortableRecord::Tombstone(record)
        if record.deleted_kind == crate::domain::library::RecordKind::Song
            && record.object_uuid == expected_uuid)
    {
        return Err(Error::conflict(
            "delete finalization tombstone identity does not match its intent",
        ));
    }

    // Keep the terminal intent and shared tombstone in one directory commit.
    // A separate intent commit would leave a crash window where recovery sees
    // Finalized but still sees the old live Song record.
    let batch_id = finalization_batch_id(operation);
    let durable = deps.control.durability().ok_or_else(|| {
        Error::unavailable(
            "library durability",
            "directory commit adapter is not configured",
        )
    })?;
    let committed = durable.read_operation(root, batch_id)?;
    let batch = if let Some(committed) = committed {
        let terminal_payload = encode(&intent)?;
        let has_finalized_intent = committed.batch.state.iter().any(|state| {
            state.kind == StateKind::DeleteIntent
                && state.id == operation.as_uuid()
                && state.payload == terminal_payload
        });
        let has_tombstone = committed.batch.records.iter().any(|envelope| {
            matches!(envelope.record(), crate::domain::library::PortableRecord::Tombstone(record)
                if record.deleted_kind == crate::domain::library::RecordKind::Song
                    && record.object_uuid == expected_uuid)
        });
        if !has_finalized_intent || !has_tombstone {
            return Err(Error::conflict(
                "saved delete finalization does not contain its terminal intent and tombstone",
            ));
        }
        committed.batch
    } else {
        let library = crate::application::scan::durable::library_id(deps, root)?;
        let mut batch = crate::domain::library::durability::LibraryBatch::new(
            batch_id,
            library,
            ChangeOrigin::LocalEdit,
            vec![tombstone],
        );
        batch.state.push(DurableStateChange::new(
            StateKind::DeleteIntent,
            operation.as_uuid(),
            encode(&intent)?,
        ));
        batch
    };
    let outcome = durable.commit(root, &batch)?;
    if outcome.status == crate::domain::library::durability::CommitStatus::SavedPendingRecovery {
        return Err(Error::saved_pending_recovery(
            OperationId::from_uuid(batch_id),
            Error::unavailable(
                "library recovery",
                "delete tombstone materialization remains pending",
            ),
        ));
    }
    Ok(Some(mutation_id))
}

/// Stable directory operation containing both the terminal intent and the
/// final tombstone. Retries reuse its frozen after-image.
#[must_use]
pub fn finalization_batch_id(operation: OperationId) -> uuid::Uuid {
    transition_id(operation, "finalized-with-tombstone-v1")
}

/// Resolve a confirmed remote final tombstone for a locally intended delete.
///
/// # Errors
/// Propagates the directory commit and reports a conflicting terminal state.
pub fn supersede_on_remote_tombstone(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    object: ObjectKey,
    now_ms: u64,
) -> Result<bool, Error> {
    let Some(mut intent) = load(deps, root, operation)? else {
        return Ok(false);
    };
    if intent.object != object {
        return Err(Error::conflict(
            "delete operation identity refers to a different object",
        ));
    }
    if intent.state != DeleteIntentState::Open {
        return Ok(intent.state == DeleteIntentState::SupersededByRemoteTombstone);
    }
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

/// Supersede every open local delete intent for an object after an authenticated
/// host tombstone has been accepted. No intent is created when the object has
/// no local undo operation.
///
/// # Errors
/// Propagates replay, payload, or directory-commit failures.
pub fn supersede_object_on_remote_tombstone(
    deps: &ScanDeps,
    root: LibraryRootId,
    object: ObjectKey,
    now_ms: u64,
) -> Result<usize, Error> {
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
    let mut latest = HashMap::new();
    for state in replay.states() {
        if state.kind != StateKind::DeleteIntent {
            continue;
        }
        let intent = serde_json::from_value::<LocalDeleteIntent>(state.payload.clone()).map_err(
            |error| {
                Error::unavailable_with_source(
                    "delete intent",
                    "the committed intent payload is unreadable",
                    error,
                )
            },
        )?;
        latest.insert(state.id, intent);
    }

    let mut superseded = 0;
    for (id, intent) in latest {
        if intent.object != object || intent.state != DeleteIntentState::Open {
            continue;
        }
        let operation = OperationId::from_uuid(id);
        if supersede_on_remote_tombstone(deps, root, operation, object, now_ms)? {
            superseded += 1;
        }
    }
    Ok(superseded)
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
    use crate::application::testing::ScanFixture;
    use crate::domain::ids::Revision;
    use crate::domain::library::{
        DeviceId, HybridLogicalClock, PortableRecord, RecordKind, TombstoneRecord,
    };

    fn tombstone(object: ObjectKey) -> crate::domain::library::PortableRecordEnvelope {
        let ObjectKey::Song(song) = object else {
            panic!("test tombstone expects a song");
        };
        crate::domain::library::PortableRecordEnvelope::new(
            PortableRecord::Tombstone(TombstoneRecord {
                object_uuid: song.as_uuid(),
                deleted_kind: RecordKind::Song,
                revision: Revision::from_u64(1),
                updated_by_device_id: DeviceId::new(),
                hlc: HybridLogicalClock::default(),
            }),
            None,
        )
        .expect("tombstone envelope")
    }

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

    #[test]
    fn remote_tombstone_supersedes_only_an_existing_open_intent() {
        let fixture = ScanFixture::new();
        let operation = OperationId::new();
        let object = ObjectKey::Song(crate::domain::ids::SongId::new());
        let open = begin(&fixture.deps, fixture.root, operation, object, 1_000)
            .expect("persist local intent");
        assert_eq!(open.state, DeleteIntentState::Open);

        assert_eq!(
            supersede_object_on_remote_tombstone(&fixture.deps, fixture.root, object, 2_000)
                .expect("remote final tombstone"),
            1
        );
        assert!(matches!(
            cancel(&fixture.deps, fixture.root, operation, object, 3_000),
            Err(Error::Conflict { .. })
        ));
        assert_eq!(
            supersede_object_on_remote_tombstone(&fixture.deps, fixture.root, object, 4_000)
                .expect("idempotent remote replay"),
            0
        );

        let absent = ObjectKey::Song(crate::domain::ids::SongId::new());
        assert_eq!(
            supersede_object_on_remote_tombstone(&fixture.deps, fixture.root, absent, 4_000)
                .expect("no local delete intent"),
            0
        );
    }

    #[test]
    fn finalization_retries_keep_the_reserved_mutation_identity() {
        let fixture = ScanFixture::new();
        let operation = OperationId::new();
        let object = ObjectKey::Song(crate::domain::ids::SongId::new());
        let open = begin(&fixture.deps, fixture.root, operation, object, 1_000)
            .expect("persist local intent");

        let first = finalize(
            &fixture.deps,
            fixture.root,
            operation,
            object,
            11_000,
            tombstone(object),
        )
        .expect("finalize")
        .expect("deadline reached");
        let retry = finalize(
            &fixture.deps,
            fixture.root,
            operation,
            object,
            12_000,
            tombstone(object),
        )
        .expect("retry finalization")
        .expect("already finalized");

        assert_eq!(first, open.final_mutation_id);
        assert_eq!(retry, first);
    }

    #[test]
    fn open_intent_survives_reopen_with_its_original_undo_deadline() {
        let fixture = ScanFixture::new();
        let operation = OperationId::new();
        let object = ObjectKey::Song(crate::domain::ids::SongId::new());
        let before_restart = begin(&fixture.deps, fixture.root, operation, object, 1_000)
            .expect("persist reversible intent");

        // A second application instance retries using the same directory and
        // operation identity, but must not restart the 10-second window.
        let after_restart = begin(&fixture.deps, fixture.root, operation, object, 5_000)
            .expect("reload intent from durable directory state");
        assert_eq!(after_restart, before_restart);
        assert_eq!(
            finalize(
                &fixture.deps,
                fixture.root,
                operation,
                object,
                10_999,
                tombstone(object),
            )
            .expect("still within the original window"),
            None
        );
        assert_eq!(
            finalize(
                &fixture.deps,
                fixture.root,
                operation,
                object,
                11_000,
                tombstone(object),
            )
            .expect("original deadline reached"),
            Some(before_restart.final_mutation_id)
        );
    }

    #[test]
    fn finalization_and_undo_race_for_one_directory_terminal_state() {
        let fixture = ScanFixture::new();
        let finalized_operation = OperationId::new();
        let finalized_object = ObjectKey::Song(crate::domain::ids::SongId::new());
        begin(
            &fixture.deps,
            fixture.root,
            finalized_operation,
            finalized_object,
            1_000,
        )
        .expect("persist first intent");

        finalize(
            &fixture.deps,
            fixture.root,
            finalized_operation,
            finalized_object,
            11_000,
            tombstone(finalized_object),
        )
        .expect("deadline finalization")
        .expect("deadline reached");
        assert!(matches!(
            cancel(
                &fixture.deps,
                fixture.root,
                finalized_operation,
                finalized_object,
                10_999,
            ),
            Err(Error::Conflict { .. })
        ));

        let cancelled_operation = OperationId::new();
        let cancelled_object = ObjectKey::Song(crate::domain::ids::SongId::new());
        begin(
            &fixture.deps,
            fixture.root,
            cancelled_operation,
            cancelled_object,
            20_000,
        )
        .expect("persist second intent");
        cancel(
            &fixture.deps,
            fixture.root,
            cancelled_operation,
            cancelled_object,
            20_001,
        )
        .expect("undo wins before deadline");
        assert!(matches!(
            finalize(
                &fixture.deps,
                fixture.root,
                cancelled_operation,
                cancelled_object,
                30_000,
                tombstone(cancelled_object),
            ),
            Err(Error::Conflict { .. })
        ));
    }
}
