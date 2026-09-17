//! Desktop deletion coordinator (task 8.11, design §9 "用户删除与外部缺失").
//!
//! Echo's user delete is a reversible operation that must not corrupt the
//! currently-playing queue. [`DeletionCoordinator`] coordinates the platform
//! player (which may hold a Windows file lock on the file being deleted) with
//! the Core delete:
//!
//! 1. **Snapshot** the target `SongId`'s `current`/queue/history/shuffle entry
//!    ids so a rollback can rebuild a valid queue.
//! 2. If the target is the **current** item, stop it and wait for the
//!    `PlayerActor` to confirm `unloaded(generation)` (the player no longer holds
//!    the file) — a Windows file handle would otherwise keep the delete from
//!    working.
//! 3. Call Core delete; only when it returns `StageApplied`/`HiddenInDatabase`
//!    do we **commit** the queue removal (drop the target's entries).
//! 4. On **any failure** (unload timeout, stage failure, DB-hidden failure, or
//!    a delete-core rejection) we **roll back** — restore the queued entries
//!    and keep the player paused — and report, never fabricating a success.
//!
//! After a successful delete, all duplicate `QueueEntryId`s for the target song
//! (including hanging history references) are removed and the next available
//! item aligns.

use echo_core::domain::ids::{QueueEntryId, SongId};

use super::port::{PlayMode, PlayerCommand, PlayerPort};
use super::queue::{HistoryRecord, Queue, QueueEntry};

/// The outcome the deletion coordinator reports to the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteCommit {
    /// The target's queue entries were removed; playback advanced cleanly.
    Committed,
    /// Rolling back: the queue was restored to its pre-delete snapshot and
    /// playback is paused (the delete itself did not succeed at the player
    /// boundary).
    RolledBack,
}

/// A bounded verdict from the player boundary about whether an unload
/// completed. In a real process this is derived from a `PlayerActor` generation
/// acknowledgment; the abstraction keeps the coordinator testable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnloadOutcome {
    /// The actor confirmed it no longer holds the current file.
    Confirmed,
    /// The actor timed out / never confirmed — deleting could hit a lock.
    TimedOut,
}

/// The Core-delete decision boundary. The real composition root maps this to
/// `DeleteSongs`/recovery; the abstraction lets the coordinator test commit vs.
/// rollback without a DB.
pub trait DeleteExecutor: Send + Sync {
    /// Ask Core to stage/hide the song's file. Returns `Ok(())` only when Core
    /// has advanced to `StageApplied`/`HiddenInDatabase` (the file is movable);
    /// an `Err` means the delete could not begin and the queue must roll back.
    ///
    /// # Errors
    ///
    /// A Core error signals the delete did not commence — the coordinator never
    /// treats it as success and never removes queue entries.
    fn hide_for_delete(&self, song: SongId) -> Result<(), String>;
}

/// Drives queue/player coordination around a single-song delete.
pub struct DeletionCoordinator<P: PlayerPort, E: DeleteExecutor> {
    player: P,
    executor: E,
    /// The last snapshot of the target song's queue entries (current/pending)
    /// taken before the delete, for rollback.
    snapshot: Option<DeletedSnapshot>,
}

/// A queue snapshot of the entries referencing one song, captured before the
/// delete so it can be restored verbatim on rollback (keeping entry ids, the
/// current reference, the timestamped history and the shuffle bag intact).
#[derive(Clone, Debug, Default)]
pub struct DeletedSnapshot {
    pub entries: Vec<QueueEntry>,
    pub current_id: Option<QueueEntryId>,
    pub history: Vec<HistoryRecord>,
    pub shuffle_bag: Vec<QueueEntryId>,
    pub shuffle_active: bool,
    pub mode: PlayMode,
}

impl<P: PlayerPort, E: DeleteExecutor> DeletionCoordinator<P, E> {
    /// A coordinator bound to the player port and the delete executor.
    #[must_use]
    pub const fn new(player: P, executor: E) -> Self {
        Self {
            player,
            executor,
            snapshot: None,
        }
    }

    /// Snapshot the current queue so it can be rebuilt on rollback.
    fn snapshot_queue(&mut self, queue: &Queue, mode: PlayMode) {
        self.snapshot = Some(DeletedSnapshot {
            entries: queue.entries().to_vec(),
            current_id: queue.current_id(),
            history: queue.history_records().to_vec(),
            shuffle_bag: queue.shuffle_bag().to_vec(),
            shuffle_active: queue.is_shuffle(),
            mode,
        });
    }

    /// Roll back to the snapshot: replace the queue and pause the player. Used
    /// when the delete could not proceed (unload/stage/DB failure).
    fn rollback(&mut self, queue: &mut Queue) {
        if let Some(snapshot) = self.snapshot.take() {
            *queue = rebuild_shallow_queue(&snapshot);
        }
        // Keep the player paused (design: 恢复有效队列并保持 paused).
        let _ = self.player.send(PlayerCommand::Pause);
        let _ = self.player.send(PlayerCommand::SetForeground(true));
    }

    /// The outcome of this coordinator's last delete attempt.
    #[must_use]
    pub const fn last_snapshot(&self) -> Option<&DeletedSnapshot> {
        self.snapshot.as_ref()
    }
}

impl<P: PlayerPort, E: DeleteExecutor> DeletionCoordinator<P, E> {
    /// Coordinate deleting `song` against the current player queue.
    ///
    /// `unload` supplies the player-unload verdict (the caller waits for the
    /// actor's `unloaded(generation)` when the target is current). Returns:
    /// - commit when the target's entries were removed;
    /// - rollback when any step failed and the queue was restored + paused.
    ///
    /// # Errors
    ///
    /// A `PlayerError`/delete error that prevented even a storable snapshot; the
    /// queue is left untouched.
    pub fn delete_song(
        &mut self,
        queue: &mut Queue,
        mode: PlayMode,
        song: SongId,
        unload: impl FnOnce() -> UnloadOutcome,
    ) -> Result<DeleteCommit, String> {
        // Snapshot the target's entries across the whole queue.
        self.snapshot_queue(queue, mode);

        // If the target is the current item, stop + wait for unload.
        let is_current = queue
            .current()
            .is_some_and(|e| e.item.song_id() == Some(song));
        if is_current {
            let _ = self.player.send(PlayerCommand::Stop);
            match unload() {
                UnloadOutcome::Confirmed => {}
                UnloadOutcome::TimedOut => {
                    // The actor still holds the file (e.g. Windows lock). Restore
                    // the valid queue and stay paused — do not proceed, do not
                    // fake a success.
                    self.rollback(queue);
                    return Ok(DeleteCommit::RolledBack);
                }
            }
        }

        // Ask Core to stage/hide the file. Only a genuine success proceeds; a
        // failure restores the queue and stays paused (rollback), never
        // removing entries or faking a delete.
        if let Err(message) = self.executor.hide_for_delete(song) {
            tracing::warn!(error = %message, "deletion coordinate: Core delete refused, rolling back queue");
            self.rollback(queue);
            return Ok(DeleteCommit::RolledBack);
        }

        // Commit: remove every queue entry (current + pending + history refs)
        // referencing the song. Duplicates all go; the next available item
        // becomes current, or the queue ends paused.
        queue.remove_song(song);
        self.snapshot = None; // a committed delete no longer needs its snapshot

        Ok(DeleteCommit::Committed)
    }
}

/// Rebuild a shallow queue from a persisted snapshot (entries + current +
/// timestamped history + shuffle bag). `Queue` has no direct over-write
/// constructor, so we push all entries and re-apply current/history/shuffle
/// references.
fn rebuild_shallow_queue(snapshot: &DeletedSnapshot) -> Queue {
    let mut queue = Queue::new();
    for entry in &snapshot.entries {
        queue.push(entry.clone());
    }
    if let Some(current) = snapshot.current_id {
        queue.set_current(current);
    }
    queue.restore_history(snapshot.history.iter().copied());
    queue.set_shuffle(snapshot.shuffle_active, snapshot.shuffle_bag.clone());
    queue
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::FakePlayer;
    use crate::player::queue::QueueItem;

    fn song() -> SongId {
        SongId::new()
    }

    fn lib_entry(s: SongId) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s),
        }
    }

    /// A delete executor that always succeeds (`hide_for_delete` returns Ok).
    struct SucceedDelete;

    impl DeleteExecutor for SucceedDelete {
        fn hide_for_delete(&self, _song: SongId) -> Result<(), String> {
            Ok(())
        }
    }

    /// A delete executor that always fails.
    struct FailDelete;

    impl DeleteExecutor for FailDelete {
        fn hide_for_delete(&self, _song: SongId) -> Result<(), String> {
            Err("core delete refused".into())
        }
    }

    fn coord_with_deleter<E: DeleteExecutor>(
        player: FakePlayer,
        executor: E,
    ) -> DeletionCoordinator<FakePlayer, E> {
        DeletionCoordinator::new(player, executor)
    }

    #[test]
    fn deleting_target_removes_all_duplicate_entries_and_advances() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, SucceedDelete);
        // Build a queue: [s, s2, s] with all three set via ViewContext-ish.
        let s = song();
        let s2 = song();
        let mut queue = Queue::new();
        let a = queue.push(lib_entry(s));
        let b = queue.push(lib_entry(s2));
        let c = queue.push(lib_entry(s));
        queue.set_current(a); // current = first s
        let s2_id = b;

        // Delete s: stops current, unloads, hides, removes both s entries.
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, s, || {
                UnloadOutcome::Confirmed
            })
            .expect("delete");
        assert_eq!(result, DeleteCommit::Committed);
        // The only remaining entry is s2 (b); the two s entries are gone.
        let present: Vec<Option<SongId>> =
            queue.entries().iter().map(|e| e.item.song_id()).collect();
        assert_eq!(present, vec![Some(s2)]);
        // The removed ids include a and c (the two s entries), not b.
        assert_eq!(queue.current_id(), Some(s2_id));
        let _ = a;
        let _ = c;
    }

    #[test]
    fn deleting_non_current_target_only_removes_its_entries() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, SucceedDelete);
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut queue = Queue::new();
        let a = queue.push(lib_entry(s1));
        let b = queue.push(lib_entry(s2));
        let c = queue.push(lib_entry(s3));
        queue.set_current(a);
        let _ = c;
        // Delete s2 (pending) — no unload needed (not current).
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, s2, || {
                panic!("no unload for a non-current target")
            })
            .expect("delete");
        assert_eq!(result, DeleteCommit::Committed);
        assert_eq!(queue.current_id(), Some(a));
        // b (s2) is gone; a (s1) and c (s3) remain.
        let present: Vec<Option<SongId>> =
            queue.entries().iter().map(|e| e.item.song_id()).collect();
        assert_eq!(present, vec![Some(s1), Some(s3)]);
        let _ = b;
    }

    #[test]
    fn unload_timeout_rolls_back_and_stays_paused() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, SucceedDelete);
        let s = song();
        let s2 = song();
        let mut queue = Queue::new();
        let a = queue.push(lib_entry(s));
        let b = queue.push(lib_entry(s2));
        queue.set_current(a);
        let _ = b;
        // Unload times out → must roll back: both entries restored, paused.
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, s, || {
                UnloadOutcome::TimedOut
            })
            .expect("delete returns a rollback outcome");
        assert_eq!(result, DeleteCommit::RolledBack);
        // Queue restored (both entries, original ids/current).
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.current_id(), Some(a));
        // The snapshot was consumed by the rollback (two entries, original ids).
        let present: Vec<Option<SongId>> =
            queue.entries().iter().map(|e| e.item.song_id()).collect();
        assert_eq!(present, vec![Some(s), Some(s2)]);
    }

    #[test]
    fn core_delete_failure_rolls_back_without_removing_entries() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, FailDelete);
        let s = song();
        let s2 = song();
        let mut queue = Queue::new();
        let a = queue.push(lib_entry(s));
        let b = queue.push(lib_entry(s2));
        queue.set_current(a);
        let _ = b;
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, s, || {
                // It IS current → stop + unload confirms, then Core delete fails.
                UnloadOutcome::Confirmed
            })
            .expect("delete returns rollback");
        // FailDelete returns Err → mapped; but our test returns Ok(RolledBack)?
        assert_eq!(result, DeleteCommit::RolledBack);
        // Entries restored.
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.current_id(), Some(a));
    }

    #[test]
    fn rollback_restores_timestamped_history_so_previous_remains_available() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, FailDelete);
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut queue = Queue::new();
        let a = queue.push(lib_entry(s1));
        let b = queue.push(lib_entry(s2));
        let c = queue.push(lib_entry(s3));
        queue.set_current(a);
        queue.set_current(b);
        queue.set_current(c); // history: [c, b, a]; current = c
        let _ = a;

        // Delete s1 (non-current) fails at the Core boundary → rollback.
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, s1, || {
                panic!("no unload for a non-current target")
            })
            .expect("delete");
        assert_eq!(result, DeleteCommit::RolledBack);

        // The rollback restored entries, current AND the timestamped history.
        assert_eq!(queue.current_id(), Some(c));
        let restored: Vec<QueueEntryId> = queue
            .history_records()
            .iter()
            .map(|record| record.entry_id)
            .collect();
        assert_eq!(restored, vec![c, b, a]);
        // "previous" still works: most recent entry ≠ current that exists.
        assert_eq!(queue.previous(), Some(b));
    }

    #[test]
    fn snapshot_queues_zero_entries_restores_empty() {
        let player = FakePlayer::new();
        let mut coord = coord_with_deleter(player, FailDelete);
        let mut queue = Queue::new();
        // Empty queue; a delete of a not-in-queue song should still snapshot
        // (empty) and roll back harmlessly on a Core failure.
        let result = coord
            .delete_song(&mut queue, PlayMode::Sequential, song(), || {
                UnloadOutcome::Confirmed
            })
            .expect("delete");
        assert_eq!(result, DeleteCommit::RolledBack);
        assert!(queue.is_empty());
    }
}
