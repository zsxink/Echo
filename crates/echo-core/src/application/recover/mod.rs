//! Import crash recovery (`RecoverOperations`, tasks 5.5 / 5.10, design §8).
//!
//! After a crash the application's *durable* truth is the operation journal
//! plus the locations each per-resource item points at: the staged copy, the
//! final target file, and the expected BLAKE3. This use case reads every
//! incomplete journal item of a root and drives each to a single, idempotent
//! terminal state by checking existence and hash at those locations — never
//! by trusting an in-memory call that "returned" before the crash.
//!
//! Per-item matrix (design §8 startup recovery rules):
//!
//! - **target already correct** (present + expected hash) → the publish is
//!   effectively applied: roll forward — for the audio resource, write the
//!   song record under the journal's **reserved** `SongId` (no new UUID), then
//!   advance `PublishApplied → DatabaseCommitted → Completed`.
//! - **valid staged copy, target absent or our own empty reservation** → retry
//!   the exclusive publish from the persisted staging path (只有暂存正确则重试
//!   exclusive publish), verify, then roll forward as above.
//! - **target occupied by different content** → `FailedRecoverable`: never
//!   overwrite; hold the target claim so nothing else claims it, and surface a
//!   conflict for manual resolution.
//! - **no correct target and no valid staging, not yet published** → roll back:
//!   clean the safe residual staged file, mark `RolledBack`, release the claim.
//! - An item that *claims* a publish (`publish_committed`) but whose file is
//!   missing is `FailedRecoverable` — the design never infers success from a
//!   missing path.
//!
//! Recovery is repeatable: re-running it is a no-op once every operation has
//! reached a terminal state, so the unique terminal state, the reserved UUID,
//! the absence of orphan final files, duplicates and ghost records all hold
//! across any number of recoveries.

use crate::application::ports::{OperationItem, OperationResourceKind, TxAccess};
use crate::application::relink::song_from_parsed;
use crate::application::scan::{parse_single_file, rewrap, FileOutcome, ScanDeps};
use crate::application::trash::{
    finalize_persisted_trash, mark_outcome_unknown, persist_state, staging_evidence,
    StagingEvidence,
};
use crate::domain::entities::LyricsSource;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, SongId};
use crate::domain::state::OperationState;
use crate::error::Error;

/// The journal `kind` of an import operation (design §8).
const IMPORT_OPERATION: &str = "import";
/// The journal `kind` of a delete operation (design §9, task 5.7).
const DELETE_OPERATION: &str = "delete";

/// Wall-clock epoch millis from a [`crate::application::ports::Clock`].
fn wall_now_ms(clock: &dyn crate::application::ports::Clock) -> Result<i64, Error> {
    let millis = clock
        .now_wall()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| {
            Error::io(
                "clock",
                std::io::Error::other(source),
                std::path::PathBuf::new(),
            )
        })?
        .as_millis();
    Ok(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// Per-item recovery result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ItemResult {
    /// The item was rolled forward/back to a durable terminal state.
    Terminal,
    /// A conflicting/foreign target that must not be overwritten; the claim
    /// stays held and the operation reports a conflict.
    Conflict,
}

/// The delete/restore recovery outcome of one operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DeleteOutcome {
    /// Whether any item ended in `FailedRecoverable` (conflict held).
    held: bool,
    /// Whether the expired operation was handed to task 5.8.
    handed_off: bool,
}

/// One operation's recovery summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryOperation {
    pub operation: OperationId,
    pub items: usize,
    /// Whether any item ended in `FailedRecoverable` (conflict held).
    pub held: bool,
    /// Whether the operation needs the runtime's `SystemTrashPort` retry.
    pub handed_off: bool,
}

/// The aggregate recovery report for one root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Operations that were not already terminal.
    pub touched: Vec<RecoveryOperation>,
}

/// The startup/retry recovery use case: finish or safely dispose every
/// incomplete import operation of a root. Blocking (file hashing, parsing,
/// publish): the runtime calls it on a worker thread before playback/watcher
/// start (task 5.10).
pub struct RecoverOperations<'a> {
    deps: &'a ScanDeps,
}

impl<'a> RecoverOperations<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps) -> Self {
        Self { deps }
    }

    /// Recover every incomplete import operation of `root`. Idempotent once
    /// stable; never fabricates a result from a missing path.
    ///
    /// # Errors
    ///
    /// Propagates only infrastructure failures while reading the journal or
    /// the filesystem; per-item conflicts are reported in the result, not
    /// raised here.
    pub fn run(&self, root: LibraryRootId) -> Result<RecoveryReport, Error> {
        let _activity = self.deps.activity.enter()?;
        if self.deps.roots.replica_enrollment_pending(root)? {
            return Ok(RecoveryReport {
                touched: Vec::new(),
            });
        }
        let items = self.deps.journal.incomplete_items(root)?;
        // Group per-operation, keeping only the kinds this task implements:
        // imports (design §8) and Echo's own deletes/restores (design §9).
        let mut ops: Vec<(OperationId, String, Vec<OperationItem>)> = Vec::new();
        for (operation, kind, item) in items {
            if kind != IMPORT_OPERATION && kind != DELETE_OPERATION {
                continue;
            }
            match ops
                .iter_mut()
                .find(|(op, k, _)| *op == operation && *k == kind)
            {
                Some((_, _, list)) => list.push(item),
                None => ops.push((operation, kind, vec![item])),
            }
        }
        let mut touched = Vec::new();
        for (operation, kind, items) in ops {
            if self
                .deps
                .roots
                .by_id(root)?
                .is_some_and(|record| record.write_safety_locked())
            {
                // Safety isolation forbids every recovery filesystem write.
                // A durable TrashApplied receipt is the one exception because
                // it needs only the database forward-roll.
                if kind == DELETE_OPERATION
                    && items
                        .iter()
                        .all(|item| item.state == OperationState::TrashApplied)
                {
                    finalize_persisted_trash(self.deps, operation, &items)?;
                    touched.push(RecoveryOperation {
                        operation,
                        items: items.len(),
                        held: false,
                        handed_off: false,
                    });
                }
                continue;
            }
            if kind == DELETE_OPERATION {
                let outcome = self.recover_delete_operation(root, operation, &items)?;
                touched.push(RecoveryOperation {
                    operation,
                    items: items.len(),
                    held: outcome.held,
                    handed_off: outcome.handed_off,
                });
            } else {
                let held = self.recover_operation(root, operation, &items)?;
                touched.push(RecoveryOperation {
                    operation,
                    items: items.len(),
                    held,
                    handed_off: false,
                });
            }
        }
        Ok(RecoveryReport { touched })
    }

    /// Recover all items of one operation. When every item reached a terminal
    /// state the operation's target claims are released; if any item is held
    /// (conflict) the claims stay reserved so the path is not re-claimed.
    fn recover_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        items: &[OperationItem],
    ) -> Result<bool, Error> {
        let mut held = false;
        for item in items {
            match self.recover_item(root, operation, item)? {
                ItemResult::Terminal => {}
                ItemResult::Conflict => held = true,
            }
        }
        if !held {
            self.deps.journal.release_claims(operation)?;
        }
        Ok(held)
    }

    /// Recover one Echo-delete operation (design §9 恢复矩阵, task 5.7).
    ///
    /// The matrix decides from the persisted item intent plus the on-disk facts
    /// at the original path and the `trash/<operation>` staged path:
    ///
    /// - `StagePending`: 原缺失而暂存匹配 → 规范化 applied; 原仍在且匹配（rename 未
    ///   发生）→ 完成暂存; 原/暂存两处证据矛盾 → 不删除任一文件（held）;
    /// - all items `StageApplied` → the hide (pending-delete + HIDDEN +
    ///   deadline) is completed if it did not commit;
    /// - `HiddenInDatabase`: 未过期 → 恢复剩余倒计时（保持等待）; 已过期 →
    ///   持久化 `TrashPending`，交给运行时的 `SystemTrashPort` 前滚;
    /// - `RestorePending`: 原路径匹配 → 规范化 restored; 否则重试恢复;
    /// - `RestoreApplied` → normalizes to `Restored`; `TrashApplied` only
    ///   finalizes from durable evidence, while a missing/unreadable pending
    ///   staging area becomes `TrashOutcomeUnknown` (never inferred success).
    fn recover_delete_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        items: &[OperationItem],
    ) -> Result<DeleteOutcome, Error> {
        if let Some(outcome) = self.recover_trash_state(root, operation, items)? {
            return Ok(outcome);
        }
        let subject_song = items.iter().find_map(|item| item.song);

        // A delete whose every file already came back (`Restored`) must also
        // bring the song record off `PendingDelete` before we call it done —
        // crash-safe undo keeps the UUID, favorite, stats and playlist
        // position (design §9). `RolledBack` is terminal too but carries no
        // restore work, so it just releases below.
        if items
            .iter()
            .all(|item| item.state == OperationState::Restored)
        {
            return self.complete_restored_delete(root, operation, subject_song);
        }
        // A fully terminal operation (RolledBack) finished its work: release
        // the claims now, exactly once (only non-terminal items are revisited,
        // so this runs once).
        if items.iter().all(|item| item.state.is_terminal()) {
            self.deps.journal.release_claims(operation)?;
            return Ok(DeleteOutcome::default());
        }
        // If the hide already committed (the song is PendingDelete), an
        // expired operation is handed to 5.8 and an unexpired one waits.
        let dump = |subject: Option<SongId>| -> Result<Option<crate::domain::entities::SongAvailability>, Error> {
            subject
                .map(|id| self.deps.songs.by_id(id))
                .transpose()?
                .flatten()
                .map(|song| Ok(song.availability()))
                .transpose()
        };
        let hidden =
            dump(subject_song)? == Some(crate::domain::entities::SongAvailability::PendingDelete);

        let mut held = self.recover_delete_items(root, operation, items)?;

        // After the per-item pass, decide the operation-level step.
        let items_after = self.deps.journal.items(operation)?;
        let all_applied = items_after
            .iter()
            .all(|item| item.state == OperationState::StageApplied);
        let all_hidden = items_after
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase);
        // The per-item pass may have just driven a mid-restore crash's files
        // home (RestorePending → Restored): finish the undo by restoring the
        // song record too (identical to the all-Restored entry point above).
        let all_restored = items_after
            .iter()
            .all(|item| item.state == OperationState::Restored);

        if all_applied && !hidden {
            // Staging completed but the hide transaction never committed:
            // complete it now (fresh undo window, the user's delete intent was
            // already confirmed and staged).
            if let Some(subject) = subject_song {
                self.hide_operation(root, operation, subject, &items_after)?;
            } else {
                held = true;
            }
            return Ok(DeleteOutcome {
                held,
                handed_off: false,
            });
        }
        if all_hidden {
            match self.deps.journal.undo_deadline(operation)? {
                Some(deadline) if wall_now_ms(&*self.deps.clock)? <= deadline => {
                    // Within the undo window: resume the remaining countdown.
                    return Ok(DeleteOutcome {
                        held,
                        handed_off: false,
                    });
                }
                Some(_) => {
                    // Expired: hand the operation to task 5.8. The minimal
                    // journal advancement is a legal Hidden→TrashPending
                    // transition per item; no SystemTrashPort call, no
                    // finalize — that is 5.8's work.
                    persist_state(
                        self.deps,
                        operation,
                        &items_after,
                        OperationState::TrashPending,
                    )?;
                    return Ok(DeleteOutcome {
                        held,
                        handed_off: true,
                    });
                }
                None => {
                    // Hidden items without a deadline is an invariant break.
                    held = true;
                }
            }
        }
        if all_restored {
            return self.complete_restored_delete(root, operation, subject_song);
        }
        if held {
            return Ok(DeleteOutcome {
                held: true,
                handed_off: false,
            });
        }
        // Not all items terminal yet and no block: drive remaining mid-flight
        // restore work on the next recovery pass (the operation stays open).
        Ok(DeleteOutcome {
            held: false,
            handed_off: false,
        })
    }

    /// Recover a delete once it reached the system-trash boundary. Missing or
    /// unreadable staging evidence is ambiguity, never success by inference.
    fn recover_trash_state(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        items: &[OperationItem],
    ) -> Result<Option<DeleteOutcome>, Error> {
        if items
            .iter()
            .all(|item| item.state == OperationState::TrashApplied)
        {
            finalize_persisted_trash(self.deps, operation, items)?;
            return Ok(Some(DeleteOutcome::default()));
        }
        let has_trash_state = items.iter().any(|item| {
            matches!(
                item.state,
                OperationState::TrashPending
                    | OperationState::TrashApplied
                    | OperationState::TrashOutcomeUnknown
            )
        });
        if !has_trash_state {
            return Ok(None);
        }
        // A mixed TrashPending/TrashApplied set is unknown: the platform acts
        // on one directory, but no operation-wide durable receipt exists.
        let evidence = staging_evidence(self.deps, root, items);
        let pending_is_intact = items
            .iter()
            .all(|item| item.state == OperationState::TrashPending)
            && evidence == StagingEvidence::Intact;
        if !pending_is_intact {
            let _ = mark_outcome_unknown(
                self.deps,
                root,
                operation,
                items,
                evidence == StagingEvidence::RootUnavailable,
            )?;
        }
        Ok(Some(DeleteOutcome {
            held: true,
            handed_off: true,
        }))
    }

    /// Drive one delete operation's per-item matrix. Returns whether any item
    /// was left held (`FailedRecoverable`): contradictory or unsupported
    /// evidence that must not be overwritten or deleted.
    fn recover_delete_items(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        items: &[OperationItem],
    ) -> Result<bool, Error> {
        let mut held = false;
        for item in items {
            match item.state {
                OperationState::StagePending => {
                    if !self.recover_stage_pending(root, operation, item)? {
                        held = true;
                    }
                }
                OperationState::StageApplied
                | OperationState::HiddenInDatabase
                | OperationState::Restored => {}
                OperationState::RestorePending => {
                    if !self.recover_restore_pending(root, operation, item)? {
                        held = true;
                    }
                }
                OperationState::RestoreApplied => {
                    self.upsert(root, operation, item, OperationState::Restored)?;
                }
                // Trash-flow states (Trash*) are 5.8's scope; any other unknown
                // mid-flight state conservatively holds the operation.
                _ => held = true,
            }
        }
        Ok(held)
    }

    /// One `StagePending` delete item: safe normalization from the original
    /// path and the trash path. Returns `true` when the item made progress.
    fn recover_stage_pending(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<bool, Error> {
        let target = self.path_hash_state(root, &item.target_path, &item.expected_hash)?;
        let staging: Option<bool> = item
            .staging_path
            .as_ref()
            .map(|path| self.path_hash_state(root, path, &item.expected_hash))
            .transpose()?
            .flatten();
        match (target, staging) {
            // 原缺失而暂存匹配 → 规范化 applied (the rename already happened).
            (None, Some(true)) => {
                self.upsert(root, operation, item, OperationState::StageApplied)?;
                Ok(true)
            }
            // 原仍在且匹配，暂存缺失 → rename 未发生，完成暂存。
            (Some(true), None) => {
                let Some(trash_path) = item.staging_path.clone() else {
                    return Err(Error::InvariantViolation {
                        why: "stage-pending delete item without a staged path".to_owned(),
                    });
                };
                let trash_name = crate::application::delete::trash_resource_name(&trash_path)?;
                self.deps
                    .fs
                    .stage_to_trash(root, operation, &item.target_path, trash_name)?;
                if self.deps.hasher.hash(root, &trash_path)? != item.expected_hash {
                    return Err(Error::CorruptMedia {
                        operation: DELETE_OPERATION.to_owned(),
                        reason: "re-staged delete copy hash differs from the journal".to_owned(),
                    });
                }
                self.upsert(root, operation, item, OperationState::StageApplied)?;
                Ok(true)
            }
            // 两处证据矛盾 (or anything else) → 不删除任一文件。
            _ => {
                self.upsert(root, operation, item, OperationState::FailedRecoverable)?;
                Ok(false)
            }
        }
    }

    /// One `RestorePending` delete item: 原路径匹配即规范化 restored, otherwise
    /// retry the restore (or hold on conflict). Returns `true` on progress.
    fn recover_restore_pending(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<bool, Error> {
        let target = self.path_hash_state(root, &item.target_path, &item.expected_hash)?;
        if target == Some(true) {
            // 原路径匹配 → the restore already landed.
            self.upsert(root, operation, item, OperationState::RestoreApplied)?;
            self.upsert(root, operation, item, OperationState::Restored)?;
            return Ok(true);
        }
        let Some(trash_path) = item.staging_path.clone() else {
            return Err(Error::InvariantViolation {
                why: "restore-pending delete item without a staged path".to_owned(),
            });
        };
        let staging = self.path_hash_state(root, &trash_path, &item.expected_hash)?;
        if staging == Some(true) {
            // A valid staged copy is intact: restore it via the SAME
            // "original free → original, occupied → safe numbered path"
            // decision as a live undo (`safe_restore_target`, shared helper),
            // so crash recovery restores exactly like undo (设计: 原路径被占用
            // 时安全编号恢复). A foreign occupant is never replaced.
            //
            // One item's conflict must never abort the whole root recovery: if
            // no safe restore path exists (the original and every numbered
            // candidate are foreign-occupied) the item is HELD
            // (`FailedRecoverable`) and reported in the result, not raised as an
            // infrastructure failure.
            let restore_target =
                match crate::application::delete::safe_restore_target(self.deps, root, item) {
                    Ok(path) => path,
                    Err(error) if error.code() == "conflict" => {
                        self.upsert(root, operation, item, OperationState::FailedRecoverable)?;
                        return Ok(false);
                    }
                    Err(error) => return Err(error),
                };
            self.deps
                .fs
                .restore_from_trash(root, &trash_path, &restore_target)?;
            if self.deps.hasher.hash(root, &restore_target)? != item.expected_hash {
                return Err(Error::CorruptMedia {
                    operation: DELETE_OPERATION.to_owned(),
                    reason: "recovered restore hash differs from the journal".to_owned(),
                });
            }
            // Record the path actually restored (the original, or the safe
            // numbered path chosen when the original was occupied), exactly
            // like the live undo's per-item journal.
            let applied = OperationItem {
                state: OperationState::RestoreApplied,
                target_path: restore_target.clone(),
                claim_key: restore_target.identity_key().to_owned(),
                ..item.clone()
            };
            let restored = OperationItem {
                state: OperationState::Restored,
                ..applied.clone()
            };
            self.deps.journal.upsert_item(operation, applied)?;
            self.deps.journal.upsert_item(operation, restored)?;
            Ok(true)
        } else {
            // No usable staged copy and no matching original: hold the item.
            self.upsert(root, operation, item, OperationState::FailedRecoverable)?;
            Ok(false)
        }
    }

    /// Complete the hide: one transaction marks the song `PendingDelete`, each
    /// item `HiddenInDatabase` and the undo deadline (now + 10s).
    fn hide_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        subject: SongId,
        items: &[OperationItem],
    ) -> Result<(), Error> {
        let _ = root;
        let deadline = wall_now_ms(&*self.deps.clock)? + crate::application::delete::UNDO_WINDOW_MS;
        let hidden: Vec<OperationItem> = items
            .iter()
            .map(|item| OperationItem {
                state: OperationState::HiddenInDatabase,
                ..item.clone()
            })
            .collect();
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.set_song_availability(
                    subject,
                    crate::domain::entities::SongAvailability::PendingDelete,
                )?;
                for item in &hidden {
                    tx.upsert_operation_item(operation, item.clone())?;
                }
                tx.set_undo_deadline(operation, deadline)?;
                Ok(())
            }))
    }

    /// Finish a delete whose every item is `Restored`: bring the song record
    /// back to `Available` if it is still hidden (crash-safe undo), then
    /// release the operation's claims at the unique terminal state.
    fn complete_restored_delete(
        &self,
        _root: LibraryRootId,
        operation: OperationId,
        subject_song: Option<SongId>,
    ) -> Result<DeleteOutcome, Error> {
        let Some(subject) = subject_song else {
            // A delete operation without a subject song is an invariant break.
            return Ok(DeleteOutcome {
                held: true,
                handed_off: false,
            });
        };
        match self.deps.songs.by_id(subject)? {
            Some(song)
                if song.availability() != crate::domain::entities::SongAvailability::Available =>
            {
                self.deps
                    .uow
                    .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                        tx.set_song_availability(
                            subject,
                            crate::domain::entities::SongAvailability::Available,
                        )?;
                        Ok(())
                    }))?;
            }
            Some(_) => {}
            None => {
                // The song record vanished — an invariant break; don't release.
                return Ok(DeleteOutcome {
                    held: true,
                    handed_off: false,
                });
            }
        }
        self.deps.journal.release_claims(operation)?;
        Ok(DeleteOutcome::default())
    }

    /// Bring one item to a durable terminal state from its persisted intent
    /// and the on-disk facts (existence + hash at target and staging).
    fn recover_item(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        let target = self.path_hash_state(root, &item.target_path, &item.expected_hash)?;
        // `transpose` yields `Option<Option<bool>>` (None = no staging path OR
        // staging absent); flatten collapses it: None = absent, Some(bool) =
        // present-with-hash-match state.
        let staging: Option<bool> = item
            .staging_path
            .as_ref()
            .map(|path| self.path_hash_state(root, path, &item.expected_hash))
            .transpose()?
            .flatten();
        // A crash between exclusive reserve and rename leaves our own zero-byte
        // placeholder; `target` then reads as `Some(false)` (different hash).
        let empty_placeholder = matches!(target, Some(false))
            && self.target_is_our_empty_placeholder(root, &item.target_path)?;

        // 1) The final file is already durably correct → roll forward.
        if target == Some(true) {
            return self.roll_forward(root, operation, item);
        }
        // 2) A valid staged copy exists → retry the exclusive publish (the
        //    publish primitive clears our own empty reservation placeholder,
        //    but never touches non-empty foreign content).
        if staging == Some(true) && (target.is_none() || empty_placeholder) {
            self.publish_staged(root, operation, item)?;
            return self.roll_forward(root, operation, item);
        }
        // 3) Nothing usable:
        //    - a non-empty foreign file occupies the target, or an item that
        //      already claimed a publish whose file vanished → held conflict;
        //    - otherwise a pre-publish item rolls back cleanly.
        if !empty_placeholder && target.is_some() {
            return self.conflict(root, operation, item);
        }
        if item.state.publish_committed() {
            return self.conflict(root, operation, item);
        }
        self.roll_back(root, operation, item)
    }

    /// Roll the audio/lyrics item forward to terminal now that its final file
    /// is durably correct: write the song record for audio (reserved UUID, in
    /// one transaction with `DatabaseCommitted`), then `Completed`.
    fn roll_forward(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        match item.kind {
            OperationResourceKind::Audio => {
                let song = item.song.ok_or_else(|| Error::InvariantViolation {
                    why: "recovered audio item without a reserved SongId".to_owned(),
                })?;
                // The record may already exist (a watcher that raced the import
                // completed it, or recovery ran before): skip, never duplicate.
                if self.deps.songs.by_id(song)?.is_none() {
                    self.commit_record(root, operation, item, song)?;
                } else {
                    // Record exists: advance the journal to DatabaseCommitted,
                    // then ensure the portable song record is there too. A
                    // crash between the DB commit and the record write (the
                    // import's materialization step, or commit_record's own)
                    // leaves a committed song without its record; re-driving
                    // it idempotently overwrites the same record file with the
                    // same revision/HLC (design §2), so recovery converges to
                    // exactly one record.
                    self.upsert(root, operation, item, OperationState::DatabaseCommitted)?;
                    self.materialize_recovered_song(root, song)?;
                }
            }
            OperationResourceKind::Lyrics => {
                // A durable whole sidecar needs no database record.
            }
        }
        self.upsert(root, operation, item, OperationState::Completed)?;
        Ok(ItemResult::Terminal)
    }

    /// Retry the publish from the journal's persisted staging path, verifying
    /// the final size/hash before counting it applied.
    fn publish_staged(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<(), Error> {
        let Some(staging_path) = item.staging_path.as_ref() else {
            return Err(Error::InvariantViolation {
                why: "recovered item without a staging path to publish".to_owned(),
            });
        };
        self.upsert(root, operation, item, OperationState::PublishPending)?;
        self.deps
            .fs
            .publish_from_staging_path(root, staging_path, &item.target_path)?;
        self.verify_target(root, item)?;
        self.upsert(root, operation, item, OperationState::PublishApplied)?;
        Ok(())
    }

    /// Verify the final file at the target matches the expected hash/size.
    fn verify_target(&self, root: LibraryRootId, item: &OperationItem) -> Result<(), Error> {
        if self.deps.hasher.hash(root, &item.target_path)? != item.expected_hash {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "recovered publish hash differs from the journal".to_owned(),
            });
        }
        Ok(())
    }

    /// Commit the song record under the reserved identity and record
    /// `DatabaseCommitted` in one transaction, exactly like a live import.
    fn commit_record(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
        song_id: crate::domain::ids::SongId,
    ) -> Result<(), Error> {
        match parse_single_file(self.deps, root, &item.target_path) {
            FileOutcome::Parsed(parsed) => {
                let embedded = parsed
                    .embedded_lyrics
                    .clone()
                    .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
                let sidecar = parsed
                    .sidecar_lyrics
                    .clone()
                    .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
                let cover = parsed.cover.clone();
                let resources = crate::application::scan_resources::verified_song_resources(
                    self.deps, root, &parsed, true,
                )?;
                let committed = OperationItem {
                    kind: OperationResourceKind::Audio,
                    state: OperationState::DatabaseCommitted,
                    song: Some(song_id),
                    source: item.source.clone(),
                    staging_path: None,
                    target_path: item.target_path.clone(),
                    expected_hash: item.expected_hash.clone(),
                    item_key: item.item_key.clone(),
                    claim_key: item.claim_key.clone(),
                };
                let added_at =
                    u64::try_from(wall_now_ms(self.deps.clock.as_ref())?).unwrap_or(u64::MAX);
                self.deps
                    .uow
                    .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                        let entity = song_from_parsed(song_id, root, &parsed.file, added_at);
                        tx.set_song_file_resources(&entity, &resources)?;
                        tx.upsert_song(&entity)?;
                        match embedded {
                            Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                            None => tx.clear_lyrics_candidate(song_id, LyricsSource::Embedded)?,
                        }
                        match sidecar {
                            Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                            None => tx.clear_lyrics_candidate(song_id, LyricsSource::Sidecar)?,
                        }
                        if let Some(cover) = cover {
                            tx.attach_cover(song_id, &cover)?;
                        }
                        tx.upsert_operation_item(operation, committed)?;
                        Ok(())
                    }))?;
                // Materialize the portable song record exactly like a live
                // import (idempotent: a crash before/after this write re-drives
                // the same revision, overwriting the same record file).
                self.materialize_recovered_song(root, song_id)?;
                Ok(())
            }
            FileOutcome::FastSkip { .. } | FileOutcome::Diagnostic(_) => Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "recovered target could not be parsed into a record".to_owned(),
            }),
        }
    }

    /// Build + write the portable song record for a recovered song, sharing the
    /// exact revision/HLC the committed row holds (design §2). Mirrors the live
    /// import materialization; a crash here leaves the record missing and the
    /// next recovery pass re-drives it idempotently.
    fn materialize_recovered_song(
        &self,
        root: LibraryRootId,
        song_id: crate::domain::ids::SongId,
    ) -> Result<(), Error> {
        let Some(song) = self.deps.songs.by_id(song_id)? else {
            return Ok(()); // Not yet committed → the roll_forward guard handles it.
        };
        crate::application::portable_materialize::ensure_control_plane_writable(
            self.deps.control.as_ref(),
            root,
        )?;
        let device = self.deps.device_id.current_device_id();
        let (revision, hlc) = crate::application::portable_materialize::committed_version(
            self.deps.sync.as_ref(),
            crate::application::portable_materialize::SONG_OBJECT_TYPE,
            &song_id.to_string(),
            device,
        )?;
        let record =
            crate::application::portable_materialize::song_record(device, hlc, revision, &song)?;
        self.deps
            .control
            .write_record(root, &crate::domain::library::PortableRecord::Song(record))?;
        Ok(())
    }

    /// Whether `target` is only Echo's own empty reservation placeholder (a
    /// crash between exclusive reserve and rename left a zero-byte file).
    fn target_is_our_empty_placeholder(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<bool, Error> {
        if !self.deps.fs.path_exists(root, target)? {
            return Ok(false);
        }
        Ok(self.deps.fs.file_meta(root, target)?.size == 0)
    }

    /// Roll a pre-publish item back: clean the residual staged file (safe),
    /// mark `RolledBack`. The claim release is decided at the operation level.
    fn roll_back(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        if let Some(staging_path) = item.staging_path.as_ref() {
            let _ = self.deps.fs.discard_staging_path(root, staging_path);
        }
        self.upsert(root, operation, item, OperationState::RolledBack)?;
        Ok(ItemResult::Terminal)
    }

    /// Hold the item on a foreign/missing conflict: `FailedRecoverable` still
    /// keeps the journal and the claim (never an automatic release or a
    /// missing-path success).
    fn conflict(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        let _ = root;
        self.upsert(root, operation, item, OperationState::FailedRecoverable)?;
        Ok(ItemResult::Conflict)
    }

    fn upsert(
        &self,
        _root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
        state: OperationState,
    ) -> Result<(), Error> {
        let next = OperationItem {
            state,
            ..item.clone()
        };
        self.deps.journal.upsert_item(operation, next)
    }

    /// `None` = absent; `Some(true)` = present with the expected hash;
    /// `Some(false)` = present with a different hash (foreign content or our
    /// own empty reservation placeholder).
    fn path_hash_state(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        expected: &str,
    ) -> Result<Option<bool>, Error> {
        if !self.deps.fs.path_exists(root, path)? {
            return Ok(None);
        }
        let hash = self.deps.hasher.hash(root, path)?;
        Ok(Some(hash == expected))
    }
}

#[cfg(test)]
mod tests;
