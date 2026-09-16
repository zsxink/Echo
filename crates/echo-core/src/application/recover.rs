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

/// The logical per-resource trash-slot name, mirroring the delete use case's
/// item key (design §9: `trash/<operation-id>/<resource>`).
const fn delete_item_key(kind: OperationResourceKind) -> &'static str {
    match kind {
        OperationResourceKind::Audio => "audio",
        OperationResourceKind::Lyrics => "lyrics",
    }
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
                self.deps.fs.stage_to_trash(
                    root,
                    operation,
                    &item.target_path,
                    delete_item_key(item.kind),
                )?;
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
                self.deps
                    .uow
                    .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                        let entity = song_from_parsed(song_id, root, &parsed.file);
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
            "song",
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
mod tests {
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::application::delete::{DeleteSongs, RestoreDeletedOperation};
    use crate::application::import::{ImportOutcome, PlanImport};
    use crate::application::ports::{
        FileMeta, ImportSource, OperationJournalRepository, PlaylistRepository, SongRepository,
    };
    use crate::application::testing::{
        small_fakes::MemoryControlPlane, FakeImportSources, ScanFixture,
    };
    use crate::domain::entities::SongAvailability;
    use crate::domain::ids::SongId;
    use crate::domain::library::PortableRecord;
    use crate::domain::media::ParsedMetadata;

    // -----------------------------------------------------------------------
    // Crash-injection harness. Wrappers around the real fakes panic at a
    // scripted point, leaving the durable journal/fs state exactly as a real
    // crash would; the test then recovers twice and asserts the invariants.
    // -----------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Phase {
        /// Crash before the side effect / state write.
        Before,
        /// Crash after the side effect ran but before its result was recorded.
        After,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Site {
        /// The streaming copy (`stage_stream`).
        Copy,
        /// The publish (`fs.publish` rename).
        Publish,
        /// The database commit (`with_tx`).
        Commit,
        /// A journal `upsert_item` of the named state (task-documented chain).
        State(&'static str),
        /// The delete/restore trash rename (`stage_to_trash`/`restore_from_trash`).
        TrashMove,
        /// The portable song-record materialization (`control.write_record`).
        RecordWrite,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Point {
        site: Site,
        phase: Phase,
    }

    /// Shared crash controller; `crash` consumes the armed point so exactly
    /// one panic is injected per arming.
    #[derive(Default)]
    struct Controller {
        armed: Mutex<Option<Point>>,
        fired: AtomicBool,
    }

    impl Controller {
        fn arm(&self, point: Point) {
            *self.armed.lock().unwrap() = Some(point);
            self.fired.store(false, Ordering::SeqCst);
        }
        fn fired(&self) -> bool {
            self.fired.load(Ordering::SeqCst)
        }
        fn crash(&self, site: &Site, phase: Phase) {
            let mut guard = self.armed.lock().unwrap();
            if guard
                .as_ref()
                .is_some_and(|p| &p.site == site && p.phase == phase)
            {
                guard.take();
                self.fired.store(true, Ordering::SeqCst);
                drop(guard);
                panic!("injected crash: {site:?} {phase:?}");
            }
        }
    }

    struct CrashFs {
        inner: Arc<dyn crate::application::ports::LibraryFileSystem>,
        ctrl: Arc<Controller>,
    }

    impl crate::application::ports::LibraryFileSystem for CrashFs {
        fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
            self.inner.enumerate(root)
        }
        fn file_meta(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<FileMeta, Error> {
            self.inner.file_meta(root, path)
        }
        fn read_head(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
            limit: u64,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_head(root, path, limit)
        }
        fn publish(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.ctrl.crash(&Site::Publish, Phase::Before);
            let result = self.inner.publish(root, staged, target);
            self.ctrl.crash(&Site::Publish, Phase::After);
            result
        }
        fn stage(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
            content: &[u8],
        ) -> Result<(), Error> {
            self.inner.stage(root, staged, content)
        }
        fn stage_stream(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
            content: &mut dyn Read,
        ) -> Result<crate::application::ports::StagedCopy, Error> {
            self.ctrl.crash(&Site::Copy, Phase::Before);
            let result = self.inner.stage_stream(root, staged, content);
            self.ctrl.crash(&Site::Copy, Phase::After);
            result
        }
        fn read_staged(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_staged(root, staged)
        }
        fn discard_staged(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
        ) -> Result<(), Error> {
            self.inner.discard_staged(root, staged)
        }
        fn path_exists(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<bool, Error> {
            self.inner.path_exists(root, path)
        }
        fn publish_from_staging_path(
            &self,
            root: LibraryRootId,
            staging_path: &RelativeMediaPath,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner
                .publish_from_staging_path(root, staging_path, target)
        }
        fn discard_staging_path(
            &self,
            root: LibraryRootId,
            staging_path: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner.discard_staging_path(root, staging_path)
        }
        fn discard_published(
            &self,
            root: LibraryRootId,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner.discard_published(root, target)
        }
        fn trash_path(
            &self,
            root: LibraryRootId,
            operation: OperationId,
            resource_key: &str,
        ) -> Result<RelativeMediaPath, Error> {
            self.inner.trash_path(root, operation, resource_key)
        }
        fn stage_to_trash(
            &self,
            root: LibraryRootId,
            operation: OperationId,
            source: &RelativeMediaPath,
            resource_key: &str,
        ) -> Result<RelativeMediaPath, Error> {
            let key = resource_key.to_owned();
            self.ctrl.crash(&Site::TrashMove, Phase::Before);
            let result = self.inner.stage_to_trash(root, operation, source, &key);
            self.ctrl.crash(&Site::TrashMove, Phase::After);
            result
        }
        fn restore_from_trash(
            &self,
            root: LibraryRootId,
            trash: &RelativeMediaPath,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.ctrl.crash(&Site::TrashMove, Phase::Before);
            let result = self.inner.restore_from_trash(root, trash, target);
            self.ctrl.crash(&Site::TrashMove, Phase::After);
            result
        }
        fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.write_capable(root)
        }
        fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
            self.inner.establish_write_capability(root)
        }
    }

    struct CrashJournal {
        inner: Arc<dyn crate::application::ports::OperationJournalRepository>,
        ctrl: Arc<Controller>,
    }

    fn state_tag(state: OperationState) -> &'static str {
        match state {
            OperationState::Planned => "planned",
            OperationState::CopyPending => "copy_pending",
            OperationState::CopyApplied => "copy_applied",
            OperationState::ValidatePending => "validate_pending",
            OperationState::Validated => "validated",
            OperationState::PublishPending => "publish_pending",
            OperationState::PublishApplied => "publish_applied",
            OperationState::DatabaseCommitted => "database_committed",
            OperationState::Completed => "completed",
            OperationState::FailedRecoverable => "failed_recoverable",
            OperationState::RolledBack => "rolled_back",
            OperationState::StagePending => "stage_pending",
            OperationState::StageApplied => "stage_applied",
            OperationState::HiddenInDatabase => "hidden_in_database",
            OperationState::RestorePending => "restore_pending",
            OperationState::RestoreApplied => "restore_applied",
            OperationState::Restored => "restored",
            OperationState::TrashPending => "trash_pending",
            OperationState::TrashApplied => "trash_applied",
            OperationState::DatabaseFinalized => "database_finalized",
            OperationState::TrashOutcomeUnknown => "trash_outcome_unknown",
        }
    }

    impl crate::application::ports::OperationJournalRepository for CrashJournal {
        fn ensure_operation(
            &self,
            operation: OperationId,
            root: LibraryRootId,
            kind: &str,
            reserved_song: Option<SongId>,
        ) -> Result<(), Error> {
            self.inner
                .ensure_operation(operation, root, kind, reserved_song)
        }
        fn item_state(
            &self,
            operation: OperationId,
            item: &str,
        ) -> Result<Option<crate::application::ports::OperationItem>, Error> {
            self.inner.item_state(operation, item)
        }
        fn upsert_item(
            &self,
            operation: OperationId,
            item: crate::application::ports::OperationItem,
        ) -> Result<(), Error> {
            let site = Site::State(state_tag(item.state));
            self.ctrl.crash(&site, Phase::Before);
            let result = self.inner.upsert_item(operation, item);
            self.ctrl.crash(&site, Phase::After);
            result
        }
        fn items(
            &self,
            operation: OperationId,
        ) -> Result<Vec<crate::application::ports::OperationItem>, Error> {
            self.inner.items(operation)
        }
        fn incomplete_items(
            &self,
            root: LibraryRootId,
        ) -> Result<
            Vec<(
                OperationId,
                String,
                crate::application::ports::OperationItem,
            )>,
            Error,
        > {
            self.inner.incomplete_items(root)
        }
        fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
            self.inner.release_claims(operation)
        }
        fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
            self.inner.set_undo_deadline(operation, deadline_ms)
        }
        fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
            self.inner.undo_deadline(operation)
        }
    }

    struct CrashUow {
        inner: Arc<dyn crate::application::ports::UnitOfWork>,
        ctrl: Arc<Controller>,
    }

    impl crate::application::ports::UnitOfWork for CrashUow {
        fn with_tx(&self, f: crate::application::ports::TxWork) -> Result<(), Error> {
            self.ctrl.crash(&Site::Commit, Phase::Before);
            let result = self.inner.with_tx(f);
            self.ctrl.crash(&Site::Commit, Phase::After);
            result
        }
    }

    /// A `MemoryControlPlane` wrapper that crashes at the portable-record write
    /// (`control.write_record`), the new materialization side effect. Like the
    /// fs/journal wrappers, it shares the underlying fake with the fixture so
    /// recovery (via the fixture's own deps) sees the durable record state.
    struct CrashControlPlane {
        inner: MemoryControlPlane,
        ctrl: Arc<Controller>,
    }

    impl crate::application::ports::ControlPlanePort for CrashControlPlane {
        fn write_manifest(
            &self,
            root: LibraryRootId,
            manifest: &crate::domain::library::LibraryManifest,
        ) -> Result<(), Error> {
            self.inner.write_manifest(root, manifest)
        }
        fn read_manifest(
            &self,
            root: LibraryRootId,
        ) -> Result<Option<crate::domain::library::LibraryManifest>, Error> {
            self.inner.read_manifest(root)
        }
        fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
            self.ctrl.crash(&Site::RecordWrite, Phase::Before);
            let result = self.inner.write_record(root, record);
            self.ctrl.crash(&Site::RecordWrite, Phase::After);
            result
        }
        fn read_record(
            &self,
            root: LibraryRootId,
            kind: crate::domain::library::RecordKind,
            object_uuid: &str,
        ) -> Result<Option<PortableRecord>, Error> {
            self.inner.read_record(root, kind, object_uuid)
        }
        fn delete_record(
            &self,
            root: LibraryRootId,
            kind: crate::domain::library::RecordKind,
            object_uuid: &str,
        ) -> Result<(), Error> {
            self.inner.delete_record(root, kind, object_uuid)
        }
        fn list_records(
            &self,
            root: LibraryRootId,
            kind: crate::domain::library::RecordKind,
        ) -> Result<Vec<String>, Error> {
            self.inner.list_records(root, kind)
        }
        fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.control_plane_usable(root)
        }
    }

    /// The crash-wired deps for one import run: the same shared fakes as the
    /// fixture, so after the injected crash the fixture's own (unwrapped) deps
    /// see the durable journal/fs state for recovery.
    fn crash_deps(
        fixture: &ScanFixture,
        ctrl: &Arc<Controller>,
    ) -> crate::application::scan::ScanDeps {
        let fs: Arc<dyn crate::application::ports::LibraryFileSystem> = Arc::new(CrashFs {
            inner: Arc::new(fixture.fs.clone()),
            ctrl: ctrl.clone(),
        });
        let journal: Arc<dyn crate::application::ports::OperationJournalRepository> =
            Arc::new(CrashJournal {
                inner: Arc::new(fixture.database.clone()),
                ctrl: ctrl.clone(),
            });
        let uow: Arc<dyn crate::application::ports::UnitOfWork> = Arc::new(CrashUow {
            inner: Arc::new(fixture.database.clone()),
            ctrl: ctrl.clone(),
        });
        let control: Arc<dyn crate::application::ports::ControlPlanePort> =
            Arc::new(CrashControlPlane {
                inner: fixture.control.clone(),
                ctrl: ctrl.clone(),
            });
        crate::application::scan::ScanDeps {
            uow,
            fs,
            journal,
            control,
            ..ScanDeps::clone(&fixture.deps)
        }
    }

    fn source(key: &str) -> ImportSource {
        ImportSource::new(key).expect("valid source handle")
    }

    /// Register a single well-formed source whose tags name
    /// `media/歌手/歌手 - 晴天.flac` and whose published file parses cleanly.
    fn wire_single(fixture: &ScanFixture, sources: &FakeImportSources, key: &str, bytes: &[u8]) {
        sources.add(key, "晴天.flac", bytes);
        fixture.metadata.set_bytes(
            bytes,
            ParsedMetadata {
                artist: Some("歌手".to_owned()),
                title: Some("晴天".to_owned()),
                ..ParsedMetadata::default()
            },
        );
        fixture.set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
    }

    /// Run `PlanImport` for the single wired source; panics if the crash did
    /// not fire (the test should fail loudly rather than silently continue).
    fn run_import_until_crash(fixture: &ScanFixture, ctrl: &Arc<Controller>, point: Point) {
        let deps = crash_deps(fixture, ctrl);
        let sources = FakeImportSources::new();
        wire_single(fixture, &sources, "sunny", b"sunny-bytes");
        let described = format!("{point:?}");
        ctrl.arm(point);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            PlanImport::new(&deps, &sources)
                .run(fixture.root, &[source("sunny")])
                .expect("batch-level success")
        }));
        assert!(
            outcome.is_err(),
            "the injection at {described} must crash the import"
        );
    }

    /// The `OperationId` of the single non-terminal import operation, found
    /// via the journal's recovery-input set (valid right after any crash).
    fn single_operation(fixture: &ScanFixture) -> OperationId {
        let items = OperationJournalRepository::incomplete_items(&fixture.database, fixture.root)
            .expect("incomplete items");
        assert_eq!(
            items.len(),
            1,
            "exactly one import operation after the crash: {items:?}"
        );
        items[0].0
    }

    /// The single distinct delete operation across all incomplete items (a
    /// delete may span several resources that share one operation id).
    fn delete_operation(fixture: &ScanFixture) -> OperationId {
        let items = OperationJournalRepository::incomplete_items(&fixture.database, fixture.root)
            .expect("incomplete items");
        assert!(
            !items.is_empty(),
            "a delete operation exists after the crash"
        );
        let operation = items[0].0;
        assert!(
            items.iter().all(|(op, _, _)| *op == operation),
            "exactly one delete operation after the crash: {items:?}"
        );
        operation
    }

    /// The reserved `SongId` the import planned (from a journal item).
    fn reserved_of(fixture: &ScanFixture, operation: OperationId) -> SongId {
        fixture
            .database
            .items(operation)
            .expect("items")
            .into_iter()
            .find_map(|item| item.song)
            .expect("reserved song id")
    }

    /// Read the durable bytes of a root-relative file.
    fn read_file(fixture: &ScanFixture, rel: &str) -> Option<Vec<u8>> {
        let base = fixture.fs.root_path(fixture.root).expect("root");
        std::fs::read(base.join(rel)).ok()
    }

    /// Assert a successfully-recovered import: exactly one song = the RESERVED
    /// UUID, the target file present once with the exact source bytes, no
    /// ghost/orphan, all items terminal, claims released exactly once.
    fn assert_unique_terminal(fixture: &ScanFixture, operation: OperationId, reserved: SongId) {
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1, "exactly one logical song, no duplicates");
        assert_eq!(
            songs[0].id(),
            reserved,
            "recovery reuses the journal-reserved UUID, never a second one"
        );
        assert_eq!(
            read_file(fixture, "media/歌手/歌手 - 晴天.flac"),
            Some(b"sunny-bytes".to_vec()),
            "the final file exists exactly once with the exact bytes"
        );
        assert!(
            read_file(fixture, songs[0].path().display()).is_some(),
            "every record has a real file (no ghost record)"
        );
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items.iter().all(|item| item.state.is_terminal()),
            "all items terminal after recovery: {items:?}"
        );
        assert_eq!(
            fixture.database.released_claims(),
            vec![operation],
            "claims released exactly once at the unique terminal state"
        );
    }

    // -----------------------------------------------------------------------
    // Recovery-matrix tests: crash at every journal-state / fs / DB-commit
    // point, recover TWICE, and assert the unique terminal state, same
    // reserved UUID, no orphan final file, no duplicate, no ghost record.
    // -----------------------------------------------------------------------

    #[test]
    fn crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice() {
        let expect_success: Vec<(Site, Phase)> = vec![
            (Site::State("copy_pending"), Phase::Before),
            (Site::State("copy_applied"), Phase::Before),
            (Site::State("validate_pending"), Phase::Before),
            (Site::State("validated"), Phase::Before),
            (Site::State("publish_pending"), Phase::Before),
            (Site::State("publish_applied"), Phase::Before),
            // `database_committed` is written atomically with the song inside
            // the DB transaction, so a crash there is exactly a DB-commit
            // interruption (covered by `Commit Before/After` below).
            (Site::State("completed"), Phase::Before),
            (Site::Publish, Phase::Before),
            (Site::Publish, Phase::After),
            (Site::Commit, Phase::Before),
            (Site::Commit, Phase::After),
            // A crash after the DB commit but before/after the portable record
            // write (the new materialization side effect): recovery must
            // re-drive the record idempotently.
            (Site::RecordWrite, Phase::Before),
            (Site::RecordWrite, Phase::After),
        ];
        for (site, phase) in expect_success {
            let fixture = ScanFixture::new();
            let ctrl = Arc::new(Controller::default());
            run_import_until_crash(&fixture, &ctrl, Point { site, phase });
            assert!(ctrl.fired(), "the crash fired at {site:?} {phase:?}");
            let operation = single_operation(&fixture);
            let reserved = reserved_of(&fixture, operation);

            // First recovery converges to the unique terminal state.
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            assert_unique_terminal(&fixture, operation, reserved);

            // Second recovery is a no-op: same UUID, still one song, no
            // duplicate file/record, claims not released twice.
            let before = fixture.all_songs();
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            let after = fixture.all_songs();
            assert_eq!(
                before, after,
                "second recovery is idempotent at {site:?} {phase:?}"
            );
            assert_eq!(after.len(), 1, "still exactly one record");
            assert_eq!(
                after[0].id(),
                reserved,
                "same reserved UUID across recoveries"
            );
            assert_eq!(fixture.database.released_claims(), vec![operation]);
        }
    }

    #[test]
    fn crash_during_record_materialization_converges_to_one_record_and_outbox() {
        // The record write happens AFTER the DB commit, so a crash there is a
        // distinctive window: the song row + outbox row are durable but the
        // `echo/records/songs/<prefix>/<uuid>.json` is not (Before) or was
        // only half-applied (After). Recovery's roll_forward must re-drive the
        // materialization idempotently — same revision, overwriting the same
        // record file — leaving exactly ONE SongRecord and ONE outbox row.
        for (site, phase) in [
            (Site::RecordWrite, Phase::Before),
            (Site::RecordWrite, Phase::After),
        ] {
            let fixture = ScanFixture::new();
            let ctrl = Arc::new(Controller::default());
            run_import_until_crash(&fixture, &ctrl, Point { site, phase });
            assert!(ctrl.fired(), "the crash fired at {site:?} {phase:?}");
            let operation = single_operation(&fixture);
            let reserved = reserved_of(&fixture, operation);

            // First recovery converges to the unique terminal state and writes
            // exactly one portable record + one outbox row.
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            assert_unique_terminal(&fixture, operation, reserved);
            let records = fixture.control.records_of(fixture.root);
            assert_eq!(
                records.len(),
                1,
                "exactly one portable record at {site:?} {phase:?}: {records:?}"
            );
            let PortableRecord::Song(song_record) = &records[0] else {
                panic!("the record is a song record");
            };
            assert_eq!(
                song_record.song_uuid,
                reserved.as_uuid(),
                "the record carries the journal-reserved identity"
            );
            assert_eq!(
                fixture.database.outbox_rows("song", &reserved.to_string()),
                vec![1],
                "exactly one sync-outbox row (revision 1)"
            );

            // Second recovery is a no-op: still one record, one outbox row.
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            assert_eq!(
                fixture.control.records_of(fixture.root).len(),
                1,
                "second recovery does not duplicate the record at {site:?} {phase:?}"
            );
            assert_eq!(
                fixture.database.outbox_rows("song", &reserved.to_string()),
                vec![1],
                "second recovery does not duplicate the outbox row"
            );
        }
    }

    #[test]
    fn copy_crash_before_any_journal_leaves_nothing_to_recover() {
        // A crash during the streaming copy happens during pre-flight, before
        // the journal envelope/claim exists: nothing is recoverable, nothing
        // is orphaned, nothing enters the library.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);
        let sources = FakeImportSources::new();
        wire_single(&fixture, &sources, "sunny", b"sunny-bytes");
        ctrl.arm(Point {
            site: Site::Copy,
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            PlanImport::new(&deps, &sources)
                .run(fixture.root, &[source("sunny")])
                .expect("batch-level success")
        }));
        assert!(outcome.is_err(), "the copy crash aborted the import");
        assert!(fixture.all_songs().is_empty(), "no record was ever created");
        let report = RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(report.touched.is_empty(), "no incomplete journal items");
        assert!(
            read_file(&fixture, "media/歌手/歌手 - 晴天.flac").is_none(),
            "no orphan final file"
        );
    }

    #[test]
    fn truncated_source_is_rejected_and_leaves_nothing() {
        // The P2 "reading mid-copy truncation" gap: a source that serves fewer
        // bytes than it described must be rejected at validation and leave no
        // final file, no record, and a rolled-back journal.
        let fixture = ScanFixture::new();
        let sources = FakeImportSources::new();
        sources.add_truncated("broken", "晴天.flac", 200, b"only-partial");
        fixture.metadata.set_bytes(
            b"only-partial",
            ParsedMetadata {
                artist: Some("歌手".to_owned()),
                title: Some("晴天".to_owned()),
                ..ParsedMetadata::default()
            },
        );
        fixture.set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
        let report = PlanImport::new(&fixture.deps, &sources)
            .run(fixture.root, &[source("broken")])
            .expect("batch-level success");
        assert!(
            matches!(report.results[0], ImportOutcome::Failed { .. }),
            "truncated source must fail, not import: {:?}",
            report.results[0]
        );
        assert!(
            fixture.all_songs().is_empty(),
            "no record from a truncated copy"
        );
        assert!(
            read_file(&fixture, "media/歌手/歌手 - 晴天.flac").is_none(),
            "no final file from a truncated copy"
        );
    }

    #[test]
    fn recovery_never_creates_a_duplicate_when_a_watcher_preempts() {
        // Watcher preemption: the audio is published and a watcher reconciles
        // it under the RESERVED UUID before the import's DB commit (task 4.9).
        // Recovery must reuse that identity, never a second record or UUID.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::State("publish_applied"),
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        let target = "media/歌手/歌手 - 晴天.flac";
        assert!(
            read_file(&fixture, target).is_some(),
            "audio already published"
        );

        // The watcher applied a record under the reserved identity (the same
        // path `apply_reserved` uses in `WatchCoordinator`).
        let mut song = crate::domain::entities::Song::new(
            reserved,
            fixture.root,
            RelativeMediaPath::new(target).unwrap(),
            crate::domain::ids::Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(b"sunny-bytes"),
            b"sunny-bytes".len() as u64,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        crate::application::ports::SongRepository::upsert(&fixture.database, &song).unwrap();
        assert_eq!(
            fixture.all_songs().len(),
            1,
            "watcher created the reserved record"
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        let songs = fixture.all_songs();
        assert_eq!(
            songs.len(),
            1,
            "no duplicate record from watcher + recovery"
        );
        assert_eq!(songs[0].id(), reserved, "the reserved UUID is the only one");
        let items = fixture.database.items(operation).expect("items");
        assert!(items.iter().all(|item| item.state.is_terminal()));
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }

    #[test]
    fn foreign_target_content_is_a_held_conflict_never_overwritten() {
        // A target already occupied by non-empty foreign content is never
        // published over: recovery holds the item FailedRecoverable, keeps the
        // claim, leaves the foreign file intact, and creates no record.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::Publish,
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        // A foreign file (different content) now sits at the target, and the
        // staged copy is lost — the worst collision recovery can face.
        let base = fixture.fs.root_path(fixture.root).expect("root");
        let items = fixture.database.items(operation).expect("items");
        for item in &items {
            if let Some(staging) = item.staging_path.as_ref() {
                let _ = std::fs::remove_file(base.join(staging.normalized()));
            }
        }
        std::fs::create_dir_all(base.join("media/歌手")).expect("mkdir artist");
        std::fs::write(base.join("media/歌手/歌手 - 晴天.flac"), b"foreign-content")
            .expect("write foreign");

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(fixture.all_songs().is_empty(), "no record, no second UUID");
        assert_eq!(
            read_file(&fixture, "media/歌手/歌手 - 晴天.flac"),
            Some(b"foreign-content".to_vec()),
            "the foreign file is never overwritten"
        );
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::FailedRecoverable),
            "conflict is held FailedRecoverable: {items:?}"
        );
        assert!(
            fixture.database.released_claims().is_empty(),
            "an unresolved conflict keeps its claim (never released)"
        );
        // Recovery is idempotent on the conflict too.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(
            read_file(&fixture, "media/歌手/歌手 - 晴天.flac"),
            Some(b"foreign-content".to_vec()),
            "second recovery still never overwrites"
        );
        assert!(fixture.all_songs().is_empty());
        let _ = reserved;
    }

    #[test]
    fn nothing_recoverable_rolls_back_and_releases_cleanly() {
        // A crash while only a claim exists (no staged copy, no target) rolls
        // back cleanly: nothing created, the item RolledBack, claim released.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::State("copy_pending"),
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        // Remove the staged copy and any target so nothing is recoverable.
        let base = fixture.fs.root_path(fixture.root).expect("root");
        for item in fixture.database.items(operation).expect("items") {
            if let Some(staging) = item.staging_path.as_ref() {
                let _ = std::fs::remove_file(base.join(staging.normalized()));
            }
        }

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(fixture.all_songs().is_empty(), "nothing created");
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::RolledBack),
            "nothing usable rolls back and releases: {items:?}"
        );
        assert_eq!(
            fixture.database.released_claims(),
            vec![operation],
            "the claim is released"
        );
        let _ = reserved;
        // Second recovery no-op (idempotent, no double release).
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }

    // -----------------------------------------------------------------------
    // Recovery-matrix tests for Echo's own delete (task 5.7, design §9).
    // Crash during stage / hide / restore, then recover TWICE to the unique
    // hidden-or-restored terminal state; a mid-restore crash must still bring
    // the song back to Available keeping UUID/favorite/stats/playlist position.
    // -----------------------------------------------------------------------

    /// Read a file from the owned `trash/<operation>/<key>` slot.
    fn read_trash(fixture: &ScanFixture, operation: OperationId, key: &str) -> Option<Vec<u8>> {
        let base = fixture.fs.root_path(fixture.root).expect("root");
        std::fs::read(
            base.join(crate::domain::library::STAGING_ROOT)
                .join("trash")
                .join(operation.as_uuid().simple().to_string())
                .join(key),
        )
        .ok()
    }

    /// The same-basename `.lrc` sidecar path beside an audio path.
    fn lrc_sibling(audio: &RelativeMediaPath) -> RelativeMediaPath {
        let normalized = audio.normalized();
        let without_extension = match normalized.rsplit_once('.') {
            Some((stem, _)) => stem,
            None => normalized,
        };
        RelativeMediaPath::new(&format!("{without_extension}.lrc"))
            .expect("derived .lrc sibling stays valid")
    }

    /// Seed one library song (favorite + play stats, optional `.lrc` sidecar)
    /// so undo-recovery can assert the preserved relationships (design §9).
    fn seed_delete_song(
        fixture: &ScanFixture,
        path: &str,
        audio: &[u8],
        lrc: Option<&[u8]>,
    ) -> SongId {
        fixture.write_file(path, audio);
        fixture.set_audio(path, "晴天", 269_000);
        let audio_path = fixture.path(path);
        let mut song = crate::domain::entities::Song::new(
            SongId::new(),
            fixture.root,
            audio_path.clone(),
            crate::domain::ids::Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(audio),
            audio.len() as u64,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        song.set_favorite(true);
        song.record_play();
        song.record_play();
        SongRepository::upsert(&fixture.database, &song).unwrap();
        if let Some(lrc_bytes) = lrc {
            let lrc_path = lrc_sibling(&audio_path);
            fixture.write_file(lrc_path.display(), lrc_bytes);
            let subject = song.id();
            let candidate = crate::domain::entities::LyricsCandidate::new(
                crate::domain::entities::LyricsSource::Sidecar,
                vec![],
                true,
            );
            fixture
                .deps
                .uow
                .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                    tx.set_lyrics_candidate(subject, &candidate)
                }))
                .unwrap();
        }
        song.id()
    }

    #[test]
    fn crash_after_delete_stage_rename_recovers_to_unique_hidden() {
        // Crash after the stage rename happened but BEFORE the `StageApplied`
        // journal write: the per-resource matrix must normalize `StagePending →
        // StageApplied` (原缺失而暂存匹配) and then finish the hide.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::State("stage_applied"),
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
        }));
        assert!(
            outcome.is_err(),
            "injection at stage_applied must crash the delete"
        );
        assert!(ctrl.fired());

        let operation = single_operation(&fixture);
        // The rename landed before the crash: original gone, trash has the
        // exact bytes.
        assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
        assert_eq!(
            read_trash(&fixture, operation, "audio"),
            Some(b"audio-bytes".to_vec())
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        let after = SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap();
        assert_eq!(
            after.availability(),
            SongAvailability::PendingDelete,
            "recovery completes the hide"
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::HiddenInDatabase),
            "unique hidden state: {items:?}"
        );
        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            None,
            "audio stays staged"
        );
        assert_eq!(
            read_trash(&fixture, operation, "audio"),
            Some(b"audio-bytes".to_vec())
        );
        assert!(
            fixture.database.undo_deadline(operation).unwrap().is_some(),
            "a fresh undo deadline is durable"
        );
        // Second recovery is an idempotent no-op on the hidden state.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::PendingDelete
        );
    }

    #[test]
    fn crash_at_the_delete_stage_rename_re_stages_then_hides() {
        // Crash at the rename itself (before it runs): original intact, trash
        // empty. Recovery re-stages from the persisted source, then hides.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::TrashMove,
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
        }));
        assert!(
            outcome.is_err(),
            "injection at the stage rename must crash the delete"
        );
        assert!(ctrl.fired());

        let operation = single_operation(&fixture);
        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            Some(b"audio-bytes".to_vec()),
            "the rename never ran"
        );
        assert!(
            read_trash(&fixture, operation, "audio").is_none(),
            "trash empty"
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::PendingDelete
        );
        assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
        assert_eq!(
            read_trash(&fixture, operation, "audio"),
            Some(b"audio-bytes".to_vec())
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(items
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase));
    }

    #[test]
    fn crash_at_the_delete_hide_commit_then_recovery_hides_audio_and_lrc() {
        // Both resources fully staged (`StageApplied`), crash at the hide DB
        // commit: recovery completes the one-transaction hide for both items.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            Some(b"lrc-bytes"),
        );
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::Commit,
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
        }));
        assert!(
            outcome.is_err(),
            "injection at the hide commit must crash the delete"
        );
        assert!(ctrl.fired());

        let operation = delete_operation(&fixture);
        let items = fixture.database.items(operation).unwrap();
        assert_eq!(items.len(), 2, "audio + lyrics both staged");
        assert!(items
            .iter()
            .all(|item| item.state == OperationState::StageApplied));
        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Available,
            "the hide never committed"
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::PendingDelete
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(items
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase));
        assert_eq!(
            read_trash(&fixture, operation, "audio"),
            Some(b"audio-bytes".to_vec())
        );
        assert_eq!(
            read_trash(&fixture, operation, "lyrics"),
            Some(b"lrc-bytes".to_vec())
        );
    }

    #[test]
    fn crash_during_restore_after_rename_recovers_song_available() {
        // Mid-undo crash AFTER the file came home but BEFORE `RestoreApplied`:
        // the matrix normalizes `RestorePending → Restored` from the matching
        // original, and the song is brought back to Available (keeping UUID,
        // favorite, stats — and the playlist position is untouched by delete).
        let fixture = ScanFixture::new();
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let playlist = crate::domain::ids::PlaylistId::new();
        fixture
            .database
            .create(playlist, fixture.root, "最爱")
            .unwrap();
        fixture.database.add_member(playlist, song, 7).unwrap();
        let outcome = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .unwrap();
        let operation = outcome.operation;

        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::State("restore_applied"),
            phase: Phase::Before,
        });
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RestoreDeletedOperation::new(&deps)
                .restore(fixture.root, operation)
                .unwrap()
        }));
        assert!(
            res.is_err(),
            "injection at restore_applied must crash the undo"
        );
        assert!(ctrl.fired());

        // The rename already landed: file home, item still `RestorePending`.
        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            Some(b"audio-bytes".to_vec())
        );
        let items = fixture.database.items(operation).unwrap();
        assert_eq!(items[0].state, OperationState::RestorePending);
        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::PendingDelete,
            "the song is still hidden mid-undo"
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        let after = SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap();
        assert_eq!(
            after.availability(),
            SongAvailability::Available,
            "undo completes"
        );
        assert_eq!(after.id(), song, "UUID preserved");
        assert!(after.favorite(), "favorite preserved");
        assert_eq!(after.play_count().as_u64(), 2, "stats preserved");
        assert_eq!(
            fixture
                .database
                .members(playlist)
                .unwrap()
                .first()
                .map(crate::domain::entities::PlaylistMember::position),
            Some(7),
            "playlist position preserved"
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(items
            .iter()
            .all(|item| item.state == OperationState::Restored));
        assert_eq!(fixture.database.released_claims(), vec![operation]);
        // Second recovery is an idempotent no-op (claims not re-released).
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }

    #[test]
    fn crash_at_the_restore_rename_recovers_song_available() {
        // Mid-undo crash BEFORE the file comes home: the staged copy still sits
        // in the trash slot. Recovery retries the restore into the original
        // path, verifies the hash, and brings the song back to Available.
        let fixture = ScanFixture::new();
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let outcome = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .unwrap();
        let operation = outcome.operation;

        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::TrashMove,
            phase: Phase::Before,
        });
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RestoreDeletedOperation::new(&deps)
                .restore(fixture.root, operation)
                .unwrap()
        }));
        assert!(
            res.is_err(),
            "injection at the restore rename must crash the undo"
        );
        assert!(ctrl.fired());

        assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
        assert_eq!(
            read_trash(&fixture, operation, "audio"),
            Some(b"audio-bytes".to_vec())
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Available
        );
        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            Some(b"audio-bytes".to_vec())
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(items
            .iter()
            .all(|item| item.state == OperationState::Restored));
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }

    #[test]
    fn foreign_occupancy_at_recover_restore_pending_holds_the_item_not_the_whole_run() {
        // A mid-undo crash leaves an item `RestorePending` with its staged copy
        // intact, and the original target later becomes re-occupied by foreign
        // content with NO safe numbered path available (every ` (n)` candidate
        // foreign too): recovery must HOLD that one item (`FailedRecoverable`),
        // never abort the whole root recovery, and still recover every unrelated
        // operation in the same pass (design §9: 两处证据矛盾时逐 item held,
        // 不删除任一文件; 一项冲突不得阻塞根目录内所有无关操作的恢复). The foreign
        // content is never replaced and the held claim stays reserved.
        let fixture = ScanFixture::new();
        let song_a = seed_delete_song(&fixture, "media/歌手/周杰伦 - 晴天.flac", b"audio-a", None);
        let song_b = seed_delete_song(
            &fixture,
            "media/歌手/林俊杰 - 不为谁而作的歌.flac",
            b"audio-b",
            None,
        );
        let op_a = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song_a)
            .unwrap()
            .operation;
        let op_b = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song_b)
            .unwrap()
            .operation;

        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);

        // Crash A's undo before the restore rename: staged copy intact, item
        // `RestorePending`, original absent.
        ctrl.arm(Point {
            site: Site::TrashMove,
            phase: Phase::Before,
        });
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                RestoreDeletedOperation::new(&deps)
                    .restore(fixture.root, op_a)
                    .unwrap()
            }))
            .is_err(),
            "injection at A's restore rename must crash the undo"
        );
        assert!(ctrl.fired());

        // Crash B's undo after the restore rename lands: original matches, item
        // `RestorePending` — it only needs normalization to `Restored`.
        ctrl.arm(Point {
            site: Site::State("restore_applied"),
            phase: Phase::Before,
        });
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                RestoreDeletedOperation::new(&deps)
                    .restore(fixture.root, op_b)
                    .unwrap()
            }))
            .is_err(),
            "injection at B's restore_applied must crash the undo"
        );
        assert!(ctrl.fired());

        // Re-occupy A's original with foreign content AND exhaust every safe
        // numbered candidate so `safe_restore_target` conflicts for A.
        fixture.write_file("media/歌手/周杰伦 - 晴天.flac", b"foreign-audio");
        for suffix in 1..=100_u32 {
            fixture.write_file(
                &format!("media/歌手/周杰伦 - 晴天 ({suffix}).flac"),
                b"foreign-numbered",
            );
        }

        // Recovery must NOT abort even though A is an unsupported conflict; B
        // (an unrelated operation) still completes in the same root pass.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        // A is held per-item: staged copy and foreign occupant untouched, claim
        // never released.
        let a_items = fixture.database.items(op_a).unwrap();
        assert!(
            a_items
                .iter()
                .all(|item| item.state == OperationState::FailedRecoverable),
            "A is held, not aborted: {a_items:?}"
        );
        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            Some(b"foreign-audio".to_vec()),
            "the foreign occupant at A's original is never replaced"
        );
        assert_eq!(
            read_trash(&fixture, op_a, "audio"),
            Some(b"audio-a".to_vec()),
            "A's staged copy is preserved for the held item"
        );
        assert!(
            !fixture.database.released_claims().contains(&op_a),
            "a held conflict never releases its claim"
        );

        // B (unrelated operation) still recovers to Restored and releases.
        let b_after = SongRepository::by_id(&fixture.database, song_b)
            .unwrap()
            .unwrap();
        assert_eq!(b_after.availability(), SongAvailability::Available);
        let b_items = fixture.database.items(op_b).unwrap();
        assert!(
            b_items
                .iter()
                .all(|item| item.state == OperationState::Restored),
            "B recovers to Restored: {b_items:?}"
        );
        assert_eq!(
            read_file(&fixture, "media/歌手/林俊杰 - 不为谁而作的歌.flac"),
            Some(b"audio-b".to_vec())
        );
        assert!(
            fixture.database.released_claims().contains(&op_b),
            "the unrelated restored operation releases its claim"
        );
    }

    #[test]
    fn contradictory_stage_evidence_holds_and_deletes_nothing() {
        // Both the original and the trash slot now hold content that
        // contradicts the journal's expected hash: the matrix must HOLD
        // (`FailedRecoverable`), never delete either file, leave the song
        // available and keep the claim (design §9: 两处证据矛盾时不删除任一文件).
        let fixture = ScanFixture::new();
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);
        ctrl.arm(Point {
            site: Site::State("stage_applied"),
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
        }));
        assert!(outcome.is_err());
        assert!(ctrl.fired());
        let operation = single_operation(&fixture);

        fixture.write_file("media/歌手/周杰伦 - 晴天.flac", b"foreign-original");
        let base = fixture.fs.root_path(fixture.root).expect("root");
        std::fs::write(
            base.join(crate::domain::library::STAGING_ROOT)
                .join("trash")
                .join(operation.as_uuid().simple().to_string())
                .join("audio"),
            b"foreign-trash",
        )
        .expect("write contradictory trash");

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();

        assert_eq!(
            read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
            Some(b"foreign-original".to_vec()),
            "the original file is never deleted"
        );
        assert!(
            read_trash(&fixture, operation, "audio").is_some(),
            "the trash file is never deleted"
        );
        let items = fixture.database.items(operation).unwrap();
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::FailedRecoverable),
            "contradictory evidence is held: {items:?}"
        );
        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Available,
            "the song is not hidden"
        );
        assert!(
            fixture.database.released_claims().is_empty(),
            "a held conflict never releases the claim"
        );
    }

    #[test]
    fn expired_hidden_delete_is_handed_to_trash_and_untouched_by_restore() {
        // An operation whose undo window expired is handed to task 5.8's trash
        // flow (`HiddenInDatabase → TrashPending`); recovery must NOT restore
        // the song or the file, and `RestoreDeletedOperation` refuses it.
        let fixture = ScanFixture::new();
        let song = seed_delete_song(
            &fixture,
            "media/歌手/周杰伦 - 晴天.flac",
            b"audio-bytes",
            None,
        );
        let outcome = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .unwrap();
        let operation = outcome.operation;
        fixture.clock.advance_ms(20_000);

        let report = RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        let recovered = report
            .touched
            .iter()
            .find(|op| op.operation == operation)
            .expect("the expired delete is touched");
        assert!(recovered.handed_off, "handed to 5.8's trash flow");
        let items = fixture.database.items(operation).unwrap();
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::TrashPending),
            "items advanced to TrashPending: {items:?}"
        );
        assert_eq!(
            SongRepository::by_id(&fixture.database, song)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::PendingDelete,
            "the song stays hidden (trash finalize is 5.8's work)"
        );
        // The undo is now refused.
        let err = RestoreDeletedOperation::new(&fixture.deps)
            .restore(fixture.root, operation)
            .unwrap_err();
        assert_eq!(err.code(), "conflict", "expired undo refused");
    }

    #[test]
    fn expired_multi_resource_handoff_is_atomic_when_persisting_trash_pending() {
        let fixture = ScanFixture::new();
        let song = seed_delete_song(
            &fixture,
            "media/artist/song.flac",
            b"audio-bytes",
            Some(b"lyrics-bytes"),
        );
        let operation = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .expect("delete")
            .operation;
        fixture.clock.advance_ms(20_000);
        fixture.database.set_fail_commit(true);

        assert!(RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .is_err());
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase));

        fixture.database.set_fail_commit(false);
        let report = RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .expect("retry handoff");
        assert!(report.touched.iter().any(|entry| entry.handed_off));
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashPending));
    }
}
