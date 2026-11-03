//! 队列内容变更规则。
//!
//! 本模块回答「队列里增删了什么」：入队与「下一首」优先区、随机轮次的重建、
//! 条目的可播状态（blocked）、清空待播，以及按条目或按歌曲移除。
//!
//! **每一个改动投影的 mutator 都必须推进队列修订号**（[`Queue::bump_revision`]），
//! 否则传输层会认为投影没变而不再下发，队列面板就会停在旧的一帧。

use echo_core::domain::ids::{QueueEntryId, SongId};

use super::{history_expired, HistoryRecord, Queue, QueueAdvance, QueueEntry, QueueItem};

impl Queue {
    /// Restore the persisted priority lane after entries have been rebuilt.
    /// Unknown, blocked, duplicate, and current IDs are discarded so a stale
    /// session can never corrupt traversal.
    pub fn set_priority(&mut self, ids: Vec<QueueEntryId>) {
        self.bump_revision();
        let current = self.current_id();
        self.priority = ids
            .into_iter()
            .filter(|id| Some(*id) != current && self.contains(*id) && !self.is_blocked(*id))
            .fold(Vec::new(), |mut unique, id| {
                if !unique.contains(&id) {
                    unique.push(id);
                }
                unique
            });
    }

    /// Enter shuffle mode. `order` is a shuffle of the pending ids that will
    /// play this round; it MAY be empty (no noise yet, or the round was
    /// already exhausted). Passing a persisted bag back does not re-shuffle.
    pub fn set_shuffle(&mut self, active: bool, order: Vec<QueueEntryId>) {
        self.bump_revision();
        self.shuffle_active = active;
        self.shuffle_bag = order;
    }

    /// Rebuild the shuffle round from the *current pending* entries, in the
    /// given order (the coordinator supplies the shuffled order of the pending
    /// ids). Appends any pending ids not already in the bag, so a partially
    /// consumed bag can be refreshed without re-shuffling already-chosen ids.
    /// Returns the new bag.
    pub fn refresh_shuffle_bag(&mut self, order: Vec<QueueEntryId>) -> Vec<QueueEntryId> {
        self.bump_revision();
        let pending = self.pending_ids();
        let mut bag: Vec<QueueEntryId> = order
            .into_iter()
            .filter(|id| pending.contains(id) && !self.shuffle_bag.contains(id))
            .collect();
        // Any pending ids not chosen this round are appended (they lived in a
        // previous round but are still unplayed).
        for id in pending {
            if !self.shuffle_bag.contains(&id) && !bag.contains(&id) {
                bag.push(id);
            }
        }
        self.shuffle_bag = std::mem::take(&mut self.shuffle_bag)
            .into_iter()
            .chain(bag)
            .collect();
        self.shuffle_bag.clone()
    }

    /// Restore timestamped history after the queue itself has been rebuilt.
    /// Invalid/missing entry IDs and expired records are ignored so a corrupt
    /// session cannot prevent startup.
    pub fn restore_history(&mut self, records: impl IntoIterator<Item = HistoryRecord>) {
        self.bump_revision();
        self.history = records
            .into_iter()
            .filter(|record| {
                self.contains(record.entry_id) && !history_expired(record.played_at_ms)
            })
            .collect();
        self.history
            .sort_by_key(|record| std::cmp::Reverse(record.played_at_ms));
    }

    /// Mark a restored entry blocked or available without changing its queue
    /// identity or position.
    pub fn set_blocked(&mut self, entry_id: QueueEntryId, blocked: bool) {
        self.bump_revision();
        if !self.contains(entry_id) {
            return;
        }
        if blocked {
            self.blocked.insert(entry_id);
            self.priority.retain(|id| *id != entry_id);
        } else {
            self.blocked.remove(&entry_id);
        }
    }

    /// Append an entry to the END of the queue (queue join). Does not disturb
    /// the current entry or the pending set's relative order.
    pub fn push(&mut self, entry: QueueEntry) -> QueueEntryId {
        self.bump_revision();
        let id = entry.id;
        self.entries.push(entry);
        if self.current_index.is_none() {
            // First item in an empty queue becomes current automatically? No —
            // the coordinator decides when to start playback. We leave
            // current_index None so the coordinator can call `start()`.
        }
        id
    }

    /// Add an entry to the tail of the FIFO "play next" lane.  Physical
    /// placement directly after the lane keeps subsequent list-loop traversal
    /// intuitive; the lane itself is authoritative for consumption order.
    ///
    /// The lane is maintained as a real contiguous slice right after the
    /// current entry (not `current_index + 1 + priority.len()`, which lands new
    /// entries *between* old lane members the moment a restore/切歌/回绕 moves
    /// the current entry ahead of stored lane members — the "下一首越插越靠后 /
    /// 乱序" defect of issue 5). If the lane has drifted apart, it is first
    /// re-located to sit flush after the current entry, then the new entry is
    /// appended to its tail.
    pub fn insert_next(&mut self, entry: QueueEntry) -> QueueEntryId {
        self.bump_revision();
        let id = entry.id;
        self.relocate_priority_lane();
        let base = self.current_index.map_or(0, |i| i + 1);
        self.entries.insert(base + self.priority.len(), entry);
        self.priority.push(id);
        id
    }

    /// Ensure the priority lane occupies `[current_index+1, current_index +
    /// priority.len())` physically, in lane order.  `set_current` (切歌、
    /// `play_queue_entry`、会话恢复) can point the current entry at a position
    /// that no longer borders the lane, or a previous remove/re-insert can
    /// strand lane members; this re-locates them so the contiguous-slice
    /// invariant holds again before the next insert.
    fn relocate_priority_lane(&mut self) {
        if self.priority.is_empty() {
            return;
        }
        let base = self.current_index.map_or(0, |i| i + 1);
        // The lane already occupies the slot right after current, in order.
        let contiguous = self
            .priority
            .iter()
            .enumerate()
            .all(|(k, id)| self.entries.get(base + k).map(|e| &e.id) == Some(id));
        if contiguous {
            return;
        }

        // Snapshot the lane entries (in lane order) so we can rebuild the slice.
        // Owned on purpose: `entries` is re-borrowed mutably below (retain), so
        // the lane snapshot must be a value, not an iterator borrowing the lane.
        #[allow(clippy::needless_collect)]
        let lane_entries: Vec<QueueEntry> = self
            .priority
            .iter()
            .filter_map(|id| self.get(*id).cloned())
            .collect();

        // Remove every lane member in one pass, counting how many sat before
        // `current_index` so the index keeps pointing at the same entry.
        let removed_before_current = self
            .entries
            .iter()
            .enumerate()
            .filter(|(pos, e)| {
                self.priority.contains(&e.id) && self.current_index.is_some_and(|ci| *pos < ci)
            })
            .count();
        self.entries.retain(|e| !self.priority.contains(&e.id));
        if let Some(ci) = self.current_index.as_mut() {
            *ci = ci.saturating_sub(removed_before_current);
        }

        // Rebase after removals (removing lane members before current moves the
        // current index forward, so the lane must follow it).
        let base = self.current_index.map_or(0, |i| i + 1);
        for (k, e) in lane_entries.into_iter().enumerate() {
            self.entries.insert(base + k, e);
        }
    }

    /// "Play next" with dedup + promote.  If an entry whose [`QueueItem`]
    /// matches `item` already exists in the queue, no new entry is created:
    /// the existing one is promoted to the very front of the FIFO lane (right
    /// after the current entry) and becomes the single priority copy.  If the
    /// item is not queued yet it is appended to the lane tail like
    /// [`Self::insert_next`].  Returns the entry id that now fronts the lane.
    ///
    /// Ordinary entries for the same song elsewhere in the queue are left
    /// untouched — only the "play next" lane dedups ("加入播放队列" still
    /// allows duplicates).  Any *lane* copies for the same playable are
    /// collapsed to the promoted one (restored sessions can hold several lane
    /// IDs for one song; spec 「优先区重复副本清理」).  A match against the
    /// *current* entry does not move it (it is playing); the current id is
    /// returned and no duplicate is added.
    pub fn promote_to_priority(&mut self, entry: QueueEntry) -> QueueEntryId {
        self.bump_revision();
        // First queued entry with the same playable identity.
        let Some(match_idx) = self
            .entries
            .iter()
            .position(|e| e.item.matches_identity(&entry.item))
        else {
            return self.insert_next(entry);
        };

        // The current entry itself: never queued against itself.
        if Some(match_idx) == self.current_index {
            return self.entries[match_idx].id;
        }

        let matched = self.entries[match_idx].clone();
        let promoted_id = matched.id;
        self.entries.remove(match_idx);
        if let Some(ci) = self.current_index {
            if match_idx < ci {
                self.current_index = Some(ci - 1);
            }
        }
        // Collapse every *lane* copy of the same playable (including this entry's
        // old position) so the promoted entry is the lane's single copy.
        let lane = std::mem::take(&mut self.priority);
        self.priority = lane
            .into_iter()
            .filter(|id| *id != promoted_id && !self.entry_matches(*id, &entry.item))
            .collect();

        let base = self.current_index.map_or(0, |i| i + 1);
        self.entries.insert(base, matched);
        self.priority.insert(0, promoted_id);
        promoted_id
    }

    /// Whether the entry `id` refers to a playable matching `item`.
    fn entry_matches(&self, id: QueueEntryId, item: &QueueItem) -> bool {
        self.get(id).is_some_and(|e| e.item.matches_identity(item))
    }

    /// Remove every pending entry (clear the upnext) WITHOUT touching the
    /// current entry or history. "清空只移除待播" (design §12, 8.6).
    pub fn clear_pending(&mut self) {
        self.bump_revision();
        if let Some(i) = self.current_index {
            let current = self.entries[i].clone();
            self.entries = vec![current];
            self.current_index = Some(0);
            let retained = self.entries[0].id;
            self.history.retain(|record| record.entry_id == retained);
            self.shuffle_bag.clear();
            self.priority.clear();
        } else if !self.entries.is_empty() {
            // No current: keep nothing (nothing is being played).
            self.entries.clear();
        }
    }

    /// Remove every queue entry referencing `song` (current, pending, and any
    /// historical id), keeping the remaining order and, if the current one was
    /// removed, advancing to the next available entry. Returns the ids removed.
    ///
    /// Used by the deletion coordinator (8.11): after a successful Echo delete
    /// all duplicate `QueueEntryId`s for the song, including hanging history
    /// references, are dropped so none try to play a now-gone file.
    pub fn remove_song(&mut self, song: SongId) -> Vec<QueueEntryId> {
        self.bump_revision();
        // The current entry (if it is a target) must also be fully removed from
        // `entries`, not merely de-pointered — design §8.11: all queue entries
        // and hanging history references for a deleted song are removed.
        let current_was_target = self
            .current()
            .is_some_and(|e| e.item.song_id() == Some(song));

        // Remove every *non-current* entry referencing the song.
        let target_ids: Vec<QueueEntryId> = self
            .entries
            .iter()
            .filter(|e| e.item.song_id() == Some(song))
            .map(|e| e.id)
            .collect();
        let mut removed: Vec<QueueEntryId> = Vec::new();
        for id in &target_ids {
            if self.remove(*id) {
                removed.push(*id);
            }
        }

        // Now, if the current was a target, advance off it and then drop its
        // (now-historical) entry from the list.
        if current_was_target {
            removed.clear(); // recompute below against the live list
            let current_id = self.current_id();
            // Advance to the next available entry (or stop).
            match self.advance_next() {
                QueueAdvance::Played(id) => removed.push(id),
                QueueAdvance::Exhausted => {
                    self.current_index = None;
                }
            }
            // Drop every remaining target entry from the live list (incl. the
            // old current that `advance_next` left in history). `advance_next`
            // only moves `current_index` and appends to the history — it never
            // touches `entries` — so reading the list here is equivalent to
            // snapshotting it before the advance.
            let leftover: Vec<QueueEntryId> = self
                .entries
                .iter()
                .filter(|e| e.item.song_id() == Some(song))
                .map(|e| e.id)
                .chain(current_id)
                .collect();
            for id in leftover {
                self.remove_direct(id);
            }
        }
        removed
    }

    /// Directly remove an entry by id from the vector (whether current or not)
    /// and drop its history reference. Unlike [`Self::remove`], this treats the
    /// current entry as removable — used by the deletion path *after* it has
    /// already advanced off the deleted current.
    pub(super) fn remove_direct(&mut self, entry_id: QueueEntryId) {
        self.bump_revision();
        if let Some(pos) = self.entries.iter().position(|e| e.id == entry_id) {
            self.entries.remove(pos);
            if let Some(ci) = self.current_index {
                if pos < ci {
                    self.current_index = Some(ci.saturating_sub(1));
                } else if pos == ci {
                    // Shouldn't normally happen (we cleared current), but guard.
                    self.current_index = None;
                }
            }
        }
        self.history.retain(|h| h.entry_id != entry_id);
        self.blocked.remove(&entry_id);
        self.priority.retain(|id| *id != entry_id);
        self.shuffle_bag.retain(|id| *id != entry_id);
    }

    /// Remove a specific entry by id if it is not the current one. Returns
    /// whether it was removed. History entry ids are removed even if the entry
    /// is gone from `entries`.
    pub fn remove(&mut self, entry_id: QueueEntryId) -> bool {
        self.bump_revision();
        if let Some(pos) = self.entries.iter().position(|e| e.id == entry_id) {
            if self.current_index == Some(pos) {
                // Cannot remove the playing entry via this path (coordinator
                // stops/advances first).
                return false;
            }
            self.entries.remove(pos);
            if let Some(ci) = self.current_index {
                if pos < ci {
                    self.current_index = Some(ci - 1);
                }
            }
            self.history.retain(|h| h.entry_id != entry_id);
            self.blocked.remove(&entry_id);
            self.priority.retain(|id| *id != entry_id);
            self.shuffle_bag.retain(|id| *id != entry_id);
            return true;
        }
        self.history.retain(|h| h.entry_id != entry_id);
        self.blocked.remove(&entry_id);
        self.priority.retain(|id| *id != entry_id);
        self.shuffle_bag.retain(|id| *id != entry_id);
        false
    }
}
