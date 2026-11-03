//! 播放顺序与推进规则。
//!
//! 本模块回答「下一首是谁」：投影的构造（[`Queue::view_entries`]）、按播放模式
//! 推进（顺序 / 随机 / 单曲循环）、上一首的历史回溯，以及播放历史的记录与裁剪。
//!
//! 队列的**内容**变更（入队、移除、优先区、随机轮次）不在这里，而在同级的
//! `mutation` 模块。

use std::collections::{HashMap, HashSet};

use echo_core::domain::ids::QueueEntryId;

use crate::player::port::PlayMode;

use super::{history_expired, now_ms, HistoryRecord, Queue, QueueAdvance, QueueEntry};

impl Queue {
    /// The pending entries ignoring shuffle (append order) — used to compute
    /// shuffle round candidates and for non-shuffle display.
    #[must_use]
    pub fn pending_in_order(&self) -> Vec<&QueueEntry> {
        self.pending()
    }

    /// The user-facing queue projection: current first, then only entries
    /// still pending in the active playback order. Storage remains append
    /// ordered so history and duplicate entry identities are never rewritten.
    ///
    /// The membership tests and the entry lookup run against indexes that are
    /// built *inside this call*, so the projection costs O(entries) instead of
    /// O(entries²). A resident index would have to be kept in sync by every
    /// mutator — one missed update would silently corrupt the projection.
    #[must_use]
    pub fn view_entries(&self) -> Vec<QueueEntry> {
        let Some(current) = self.current() else {
            return self.entries.clone();
        };
        let pending = self.circular_pending_ids();
        let pending_set: HashSet<QueueEntryId> = pending.iter().copied().collect();
        let shuffle_bag_set: HashSet<QueueEntryId> = self.shuffle_bag.iter().copied().collect();
        let by_id: HashMap<QueueEntryId, &QueueEntry> =
            self.entries.iter().map(|entry| (entry.id, entry)).collect();

        let order: Vec<QueueEntryId> = if self.shuffle_active {
            self.shuffle_bag
                .iter()
                .copied()
                .filter(|id| pending_set.contains(id))
                .chain(
                    pending
                        .iter()
                        .copied()
                        .filter(|id| !shuffle_bag_set.contains(id)),
                )
                .collect()
        } else {
            pending
        };

        let mut view = vec![current.clone()];
        let mut emitted: HashSet<QueueEntryId> = HashSet::with_capacity(self.entries.len() + 1);
        emitted.insert(current.id);
        for id in self.priority.iter().copied().chain(order) {
            if emitted.contains(&id) || self.is_blocked(id) {
                continue;
            }
            if let Some(entry) = by_id.get(&id) {
                emitted.insert(id);
                view.push((*entry).clone());
            }
        }
        view
    }

    /// Mark `entry_id` as the current one and record it in history. Used by the
    /// coordinator when it starts/advances to an entry. A non-pending entry id
    /// is a no-op (returns None).
    pub fn set_current(&mut self, entry_id: QueueEntryId) -> Option<QueueEntryId> {
        self.bump_revision();
        let pos = self.entries.iter().position(|e| e.id == entry_id)?;
        self.current_index = Some(pos);
        self.record_history(entry_id);
        Some(entry_id)
    }

    /// Advance to the next pending entry in append order (sequential mode).
    /// Returns [`QueueAdvance::Played`] with the new current, or
    /// [`QueueAdvance::Exhausted`] when there is no pending entry.
    pub fn advance_next(&mut self) -> QueueAdvance {
        self.bump_revision();
        let next_index = match self.current_index {
            Some(i) if i + 1 < self.entries.len() => i + 1,
            None if !self.entries.is_empty() => 0,
            _ => return QueueAdvance::Exhausted,
        };
        let id = self.entries[next_index].id;
        self.current_index = Some(next_index);
        self.record_history(id);
        QueueAdvance::Played(id)
    }

    /// Consume the first valid manual priority entry. Manual previous and
    /// error skip intentionally never call this method.
    pub fn advance_priority(&mut self) -> Option<QueueEntryId> {
        self.bump_revision();
        while let Some(id) = self.priority.first().copied() {
            self.priority.remove(0);
            if self.contains(id) && !self.is_blocked(id) {
                self.set_current(id);
                return Some(id);
            }
        }
        None
    }

    /// Wrap around to the first entry (列表循环回卷): the sequential tail
    /// loops back to the head and playback continues. A non-empty queue
    /// therefore never exhausts in sequential mode; a single-entry queue
    /// loops on itself (the caller reloads it, like repeat-one).
    fn wrap_to_first(&mut self) -> Option<QueueEntryId> {
        let first = self.entries.first()?.id;
        self.current_index = Some(0);
        self.record_history(first);
        Some(first)
    }

    /// Advance to the next entry per the play mode (task 8.6).
    ///
    /// - `Sequential` (列表循环): take the next pending entry in append order;
    ///   when the tail is reached, **wrap around to the first entry** and keep
    ///   playing — the queue is a loop, not a playlist that stops (设计: 默认
    ///   列表循环). The caller's failed-round guard (coordinator) stops the
    ///   loop when every entry failed.
    /// - `Shuffle`: pop the next id from the shuffle bag. When the bag is
    ///   empty the caller refreshes it via [`Self::refresh_shuffle_bag`]
    ///   before the next call; otherwise this returns `Exhausted`.
    /// - `RepeatOne`: returns the *current* id (repeat the same entry); the
    ///   caller re-loads it. (Error skip, task 8.7, bypasses this.)
    ///
    /// Returns the id advanced to, or `None` when there is nothing to play.
    pub fn advance_in_mode(&mut self, mode: PlayMode) -> Option<QueueEntryId> {
        self.bump_revision();
        match mode {
            PlayMode::Sequential => match self.advance_next() {
                QueueAdvance::Played(id) => Some(id),
                QueueAdvance::Exhausted => self.wrap_to_first(),
            },
            PlayMode::Shuffle => {
                let id = self.shuffle_bag.first().copied()?;
                self.shuffle_bag.remove(0);
                let Some(pos) = self.entries.iter().position(|e| e.id == id) else {
                    // Id no longer present (removed while shuffled): skip and
                    // continue popping.
                    return self.advance_in_mode(PlayMode::Shuffle);
                };
                self.current_index = Some(pos);
                self.record_history(id);
                Some(id)
            }
            PlayMode::RepeatOne => self.current_id(),
        }
    }

    /// Move to the previous entry (history): the most recent distinct entry
    /// before the current one. Returns its id, or `None` if there is no prior.
    /// # Panics
    ///
    /// Panics only if internal history is corrupted after `prune_history` has
    /// established that the selected entry remains in the queue.
    pub fn previous(&mut self) -> Option<QueueEntryId> {
        self.bump_revision();
        let current = self.current_id()?;
        // Find the most recent history entry different from current that still
        // exists in the queue.
        self.prune_history();
        let index = self.history.iter().position(|h| {
            h.entry_id != current && self.entries.iter().any(|e| e.id == h.entry_id)
        })?;
        let prev = self.history[index].entry_id;
        let pos = self
            .entries
            .iter()
            .position(|e| e.id == prev)
            .expect("history entry in queue");
        self.current_index = Some(pos);
        Some(prev)
    }

    /// Move through the canonical list order in reverse, wrapping from the
    /// first entry to the last. This deliberately does not consume or reorder
    /// the manual priority lane.
    pub fn previous_in_loop(&mut self) -> Option<QueueEntryId> {
        self.bump_revision();
        let current = self.current_index?;
        if self.entries.is_empty() {
            return None;
        }
        let previous = if current == 0 {
            self.entries.len() - 1
        } else {
            current - 1
        };
        let id = self.entries[previous].id;
        self.current_index = Some(previous);
        self.record_history(id);
        Some(id)
    }

    /// Every non-current entry in the next list-loop order, including entries
    /// before the current storage position which will play after wrapping.
    fn circular_pending_ids(&self) -> Vec<QueueEntryId> {
        let Some(current) = self.current_index else {
            return self.entries.iter().map(|entry| entry.id).collect();
        };
        self.entries[current + 1..]
            .iter()
            .chain(self.entries[..current].iter())
            .map(|entry| entry.id)
            .collect()
    }

    fn record_history(&mut self, entry_id: QueueEntryId) {
        self.prune_history();
        self.history.insert(
            0,
            HistoryRecord {
                entry_id,
                played_at_ms: now_ms(),
            },
        );
    }

    fn prune_history(&mut self) {
        self.history
            .retain(|record| !history_expired(record.played_at_ms));
    }
}
