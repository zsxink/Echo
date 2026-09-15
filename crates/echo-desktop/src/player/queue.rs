//! Playback queue, history and the coordinator rules (task 8.5, 8.6, 8.7).
//!
//! The queue lives entirely on the desktop player layer (design §12: 队列与播放
//! 会话留在桌面层). [`QueueEntryId`] from `echo-core` is the stable identity of
//! one *entry*; the same [`SongId`] appended twice gets two independent entry
//! IDs and is never collapsed (design §12: 同一 SongId 的每次追加拥有不同
//! queue entry ID，不能折叠).
//!
//! [`Queue`] is a pure, testable container — current entry, pending entries,
//! history — with no I/O. [`PlaybackCoordinator`] uses it and the player port
//! to drive actual playback.
//!
//! **Helpful invariants:**
//! - `current()` is the item being played (or `None` when stopped).
//! - `pending()` are the upcoming items, played in order (or per shuffle bag).
//! - `history` is a bounded record of recently played entry IDs (for "previous").
//! - Every mutation that touches an entry works by `QueueEntryId` — never by
//!   `SongId`-based identity — so duplicate songs stay distinct.

use echo_core::domain::ids::{QueueEntryId, SongId};

use super::port::PlayMode;

/// One entry in the queue. The identity is the [`QueueEntryId`]; the item is
/// either a library song (by `SongId`) or a session-only temporary file.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub item: QueueItem,
}

/// What a queue entry refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueItem {
    /// A library song, resolved by stable `SongId` to its file (task 9.2).
    Library(SongId),
    /// A session-only temporary item with its validated absolute path and a
    /// snapshot of its metadata. Not persisted; filtered on save (8.9).
    Temporary(TemporaryItem),
}

/// A temporary (session-only) playback item outside the active library.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporaryItem {
    /// Display name (never a filesystem path).
    pub display_name: String,
    /// Validated absolute path to the local media file.
    pub path: std::path::PathBuf,
    /// Best-effort duration (seconds), `None` if unknown yet.
    pub duration: Option<f64>,
    /// Whether this temporary item's file is on the *currently active* root
    /// (i.e. it was opened from a non-active root / outside the library).
    pub on_active_root: bool,
}

impl QueueItem {
    /// The `SongId` when this item is a library song.
    #[must_use]
    pub const fn song_id(&self) -> Option<SongId> {
        match self {
            Self::Library(id) => Some(*id),
            Self::Temporary(_) => None,
        }
    }
}

/// A pure queue container: current + pending + bounded history.
///
/// `Queue` is deliberately free of I/O and of playback logic; it models only
/// *identity and order* so its rules are unit-testable. The coordinator (and
/// later the session) drive it.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    /// All entries in append order, whether current, pending or historical.
    /// Current is `entries[current_index]`; pending follow; history is tracked
    /// separately by entry id for "previous".
    entries: Vec<QueueEntry>,
    current_index: Option<usize>,
    /// Recently played entry ids, most-recent first, capped.
    history: Vec<QueueEntryId>,
    history_cap: usize,
    /// The active shuffle round: a FIFO of `QueueEntryId`s in the order they
    /// will play, in shuffle mode. Each id appears at most once per round, so
    /// a round never repeats an entry (design §8.6). Built by the coordinator
    /// from the pending ids at the time shuffle is entered (or when the bag
    /// empties); on session restore the coordinator passes the persisted bag
    /// back directly, so it is *not* re-shuffled (验证: 恢复后不重洗).
    shuffle_bag: Vec<QueueEntryId>,
    /// Whether shuffle mode is currently active. The bag is only consulted in
    /// shuffle mode; switching modes never rebuilds it (切模式不丢当前项).
    shuffle_active: bool,
}

/// A result of advancing/removing that the caller should surface.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueAdvance {
    /// Clean advance to a new entry.
    Played(QueueEntryId),
    /// The queue has no more pending entries.
    Exhausted,
}

impl Queue {
    /// A new empty queue with a default history cap.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            current_index: None,
            history: Vec::new(),
            history_cap: 100,
            shuffle_bag: Vec::new(),
            shuffle_active: false,
        }
    }

    /// Whether shuffle mode is active (the bag is consulted on advance).
    #[must_use]
    pub fn is_shuffle(&self) -> bool {
        self.shuffle_active
    }

    /// The current shuffle bag (ids remaining in this round, in play order).
    #[must_use]
    pub fn shuffle_bag(&self) -> &[QueueEntryId] {
        &self.shuffle_bag
    }

    /// Enter shuffle mode. `order` is a shuffle of the pending ids that will
    /// play this round; it MAY be empty (no noise yet, or the round was
    /// already exhausted). Passing a persisted bag back does not re-shuffle.
    pub fn set_shuffle(&mut self, active: bool, order: Vec<QueueEntryId>) {
        self.shuffle_active = active;
        self.shuffle_bag = order;
    }

    /// Rebuild the shuffle round from the *current pending* entries, in the
    /// given order (the coordinator supplies the shuffled order of the pending
    /// ids). Appends any pending ids not already in the bag, so a partially
    /// consumed bag can be refreshed without re-shuffling already-chosen ids.
    /// Returns the new bag.
    pub fn refresh_shuffle_bag(&mut self, order: Vec<QueueEntryId>) -> Vec<QueueEntryId> {
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

    /// The pending entries ignoring shuffle (append order) — used to compute
    /// shuffle round candidates and for non-shuffle display.
    #[must_use]
    pub fn pending_in_order(&self) -> Vec<&QueueEntry> {
        self.pending()
    }

    /// The number of entries (current + pending; history entries that were
    /// removed are not counted).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the queue is empty (no current and no pending).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The current entry, if any.
    #[must_use]
    pub fn current(&self) -> Option<&QueueEntry> {
        self.current_index.and_then(|i| self.entries.get(i))
    }

    /// The current entry's id, if any.
    #[must_use]
    pub fn current_id(&self) -> Option<QueueEntryId> {
        self.current().map(|e| e.id)
    }

    /// Everything after the current entry (pending play order).
    #[must_use]
    pub fn pending(&self) -> Vec<&QueueEntry> {
        match self.current_index {
            Some(i) if i + 1 < self.entries.len() => self.entries[i + 1..].iter().collect(),
            // No current (empty) → nothing before the first entry is "pending"
            // in the defined sense; return all but the first so a fresh queue
            // with no current plays from the head.
            _ if self.entries.len() > 1 => self.entries[1..].iter().collect(),
            _ => Vec::new(),
        }
    }

    /// All pending entry ids.
    #[must_use]
    pub fn pending_ids(&self) -> Vec<QueueEntryId> {
        self.pending().into_iter().map(|e| e.id).collect()
    }

    /// The recent history (most-recent first).
    #[must_use]
    pub fn history(&self) -> &[QueueEntryId] {
        &self.history
    }

    /// Look up an entry by id (covers current, pending and history entries
    /// that are still in the `entries` list).
    #[must_use]
    pub fn get(&self, id: QueueEntryId) -> Option<&QueueEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// All entries in append order — current, pending, and any historical
    /// entry still present in the list. Used by session persistence (8.9) to
    /// snapshot the full queue with independent entry ids intact.
    #[must_use]
    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    /// Append an entry to the END of the queue (queue join). Does not disturb
    /// the current entry or the pending set's relative order.
    pub fn push(&mut self, entry: QueueEntry) -> QueueEntryId {
        let id = entry.id;
        self.entries.push(entry);
        if self.current_index.is_none() {
            // First item in an empty queue becomes current automatically? No —
            // the coordinator decides when to start playback. We leave
            // current_index None so the coordinator can call `start()`.
        }
        id
    }

    /// Insert `entry` immediately after the current entry (or at the head if
    /// there is no current). "下一首播放" (design §12).
    pub fn insert_next(&mut self, entry: QueueEntry) -> QueueEntryId {
        let id = entry.id;
        let insert_at = match self.current_index {
            Some(i) => i + 1,
            None => 0,
        };
        self.entries.insert(insert_at, entry);
        // A None current that now has a head item: leave for the coordinator.
        id
    }

    /// Mark `entry_id` as the current one and record it in history. Used by the
    /// coordinator when it starts/advances to an entry. A non-pending entry id
    /// is a no-op (returns None).
    pub fn set_current(&mut self, entry_id: QueueEntryId) -> Option<QueueEntryId> {
        let pos = self.entries.iter().position(|e| e.id == entry_id)?;
        self.current_index = Some(pos);
        self.history.insert(0, entry_id);
        self.history.truncate(self.history_cap);
        Some(entry_id)
    }

    /// Advance to the next pending entry in append order (sequential mode).
    /// Returns [`QueueAdvance::Played`] with the new current, or
    /// [`QueueAdvance::Exhausted`] when there is no pending entry.
    pub fn advance_next(&mut self) -> QueueAdvance {
        let next_index = match self.current_index {
            Some(i) if i + 1 < self.entries.len() => i + 1,
            None if !self.entries.is_empty() => 0,
            _ => return QueueAdvance::Exhausted,
        };
        let id = self.entries[next_index].id;
        self.current_index = Some(next_index);
        self.history.insert(0, id);
        self.history.truncate(self.history_cap);
        QueueAdvance::Played(id)
    }

    /// Wrap around to the first entry (列表循环回卷): the sequential tail
    /// loops back to the head and playback continues. A non-empty queue
    /// therefore never exhausts in sequential mode; a single-entry queue
    /// loops on itself (the caller reloads it, like repeat-one).
    fn wrap_to_first(&mut self) -> Option<QueueEntryId> {
        let first = self.entries.first()?.id;
        self.current_index = Some(0);
        self.history.insert(0, first);
        self.history.truncate(self.history_cap);
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
        match mode {
            PlayMode::Sequential => match self.advance_next() {
                QueueAdvance::Played(id) => Some(id),
                QueueAdvance::Exhausted => self.wrap_to_first(),
            },
            PlayMode::Shuffle => {
                let id = self.shuffle_bag.first().copied()?;
                self.shuffle_bag.remove(0);
                let pos = match self.entries.iter().position(|e| e.id == id) {
                    Some(p) => p,
                    None => {
                        // Id no longer present (removed while shuffled): skip
                        // and continue popping.
                        return self.advance_in_mode(PlayMode::Shuffle);
                    }
                };
                self.current_index = Some(pos);
                self.history.insert(0, id);
                self.history.truncate(self.history_cap);
                Some(id)
            }
            PlayMode::RepeatOne => self.current_id(),
        }
    }

    /// Move to the previous entry (history): the most recent distinct entry
    /// before the current one. Returns its id, or `None` if there is no prior.
    pub fn previous(&mut self) -> Option<QueueEntryId> {
        let current = self.current_id()?;
        // Find the most recent history entry different from current that still
        // exists in the queue.
        let index = self
            .history
            .iter()
            .position(|&h| h != current && self.entries.iter().any(|e| e.id == h))?;
        let prev = self.history[index];
        let pos = self
            .entries
            .iter()
            .position(|e| e.id == prev)
            .expect("history entry in queue");
        self.current_index = Some(pos);
        Some(prev)
    }

    /// Remove every pending entry (clear the upnext) WITHOUT touching the
    /// current entry or history. "清空只移除待播" (design §12, 8.6).
    pub fn clear_pending(&mut self) {
        if let Some(i) = self.current_index {
            self.entries.truncate(i + 1);
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
            let target_remaining: Vec<QueueEntryId> = self
                .entries
                .iter()
                .filter(|e| e.item.song_id() == Some(song))
                .map(|e| e.id)
                .collect();
            // Advance to the next available entry (or stop).
            match self.advance_next() {
                QueueAdvance::Played(id) => removed.push(id),
                QueueAdvance::Exhausted => {
                    self.current_index = None;
                }
            }
            // Drop every remaining target entry from the list (incl. the old
            // current that `advance_next` left in history).
            let leftover: Vec<QueueEntryId> =
                target_remaining.into_iter().chain(current_id).collect();
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
    fn remove_direct(&mut self, entry_id: QueueEntryId) {
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
        self.history.retain(|&h| h != entry_id);
    }

    /// Remove a specific entry by id if it is not the current one. Returns
    /// whether it was removed. History entry ids are removed even if the entry
    /// is gone from `entries`.
    pub fn remove(&mut self, entry_id: QueueEntryId) -> bool {
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
            self.history.retain(|&h| h != entry_id);
            return true;
        }
        self.history.retain(|&h| h != entry_id);
        false
    }

    /// Whether an entry (by id) is currently pending (not current).
    #[must_use]
    pub fn is_pending(&self, entry_id: QueueEntryId) -> bool {
        match self.current_index {
            Some(i) => self.entries[i + 1..].iter().any(|e| e.id == entry_id),
            None => false,
        }
    }

    /// Whether an entry id exists anywhere in the queue (current, pending, or
    /// historical-but-still-present).
    #[must_use]
    pub fn contains(&self, entry_id: QueueEntryId) -> bool {
        self.entries.iter().any(|e| e.id == entry_id)
    }

    /// The total number of distinct `SongId`s currently in the queue
    /// (current + pending) — a helper for callers that need to know how many
    /// distinct songs are queued without collapsing duplicated entries.
    #[must_use]
    pub fn distinct_song_ids(&self) -> Vec<SongId> {
        let mut seen = Vec::new();
        for e in &self.entries {
            if let Some(s) = e.item.song_id() {
                if !seen.contains(&s) {
                    seen.push(s);
                }
            }
        }
        seen
    }
}

/// A view-context play request: the coordinator derives the queue from a
/// deterministic list of songs (server-resolved by the Core playback-context
/// request) starting at a selected song. Task 8.5: 视图上下文.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewContext {
    /// The deterministic song list to build the queue from (all songs of the
    /// view, in play order). Duplicate `SongId`s are kept as separate entries.
    pub songs: Vec<SongId>,
    /// The song to start at (its index within `songs`).
    pub selected_index: usize,
}

impl ViewContext {
    /// Build a queue from a view context: each song becomes a `Library` entry
    /// with an independent `QueueEntryId`, and the selected song is made
    /// current. Returns the queue.
    #[must_use]
    pub fn build_queue(&self) -> Queue {
        let mut q = Queue::new();
        let entries: Vec<QueueEntry> = self
            .songs
            .iter()
            .map(|s| QueueEntry {
                id: QueueEntryId::new(),
                item: QueueItem::Library(*s),
            })
            .collect();
        let selected = self.selected_index.min(self.songs.len().saturating_sub(1));
        for (i, e) in entries.iter().enumerate() {
            q.push(e.clone());
            if i == selected {
                q.set_current(e.id);
            }
        }
        q
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(s: SongId) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s),
        }
    }

    fn song() -> SongId {
        SongId::new()
    }

    #[test]
    fn duplicate_song_ids_get_independent_entry_ids() {
        let s = song();
        let mut q = Queue::new();
        let a = q.push(lib(s));
        let b = q.push(lib(s));
        let c = q.push(lib(s));
        // Three distinct QueueEntryIds for the same SongId.
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(q.distinct_song_ids(), vec![s]);
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn insert_next_goes_immediately_after_current() {
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut q = Queue::new();
        q.push(lib(s1));
        let s3_id = q.push(lib(s3));
        q.set_current(s3_id); // s3 is current; s1 is behind it (in history)

        // Insert s2 as "next" → must go immediately after s3.
        let inserted = q.insert_next(lib(s2));
        let cur = q.current().unwrap();
        assert_eq!(cur.item.song_id(), Some(s3));
        // pending is [s2], nothing else (s1 is historical/behind).
        assert_eq!(q.pending_ids(), vec![inserted]);
        assert_eq!(q.pending()[0].item.song_id(), Some(s2));
    }

    #[test]
    fn advance_next_plays_in_order_and_exhausts() {
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        q.push(lib(s1));
        q.push(lib(s2));
        assert_eq!(q.current(), None);

        assert_eq!(q.advance_next(), QueueAdvance::Played(q.entries[0].id));
        assert_eq!(q.current().unwrap().item.song_id(), Some(s1));
        assert_eq!(q.advance_next(), QueueAdvance::Played(q.entries[1].id));
        assert_eq!(q.current().unwrap().item.song_id(), Some(s2));
        assert_eq!(q.advance_next(), QueueAdvance::Exhausted);
    }

    #[test]
    fn clear_pending_keeps_current_and_history() {
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        let c = q.push(lib(s1));
        let p = q.push(lib(s2));
        q.set_current(c);
        assert_eq!(q.pending_ids(), vec![p]);

        q.clear_pending();
        // clear_pending removes pending; current stays.
        assert!(!q.is_pending(p));
        assert!(q.pending().is_empty());
        assert_eq!(q.current().map(|e| e.id), Some(c));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn remove_pending_only() {
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        let c = q.push(lib(s1));
        let p = q.push(lib(s2));
        q.set_current(c);

        assert!(q.remove(p)); // pending removed
        assert!(!q.contains(p));
        assert_eq!(q.current().map(|e| e.id), Some(c));

        // Cannot remove the current entry.
        assert!(!q.remove(c));
    }

    #[test]
    fn previous_returns_history_entry() {
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s1));
        let b = q.push(lib(s2));
        let _c = q.push(lib(s3));
        q.set_current(a);
        q.set_current(b); // history: [b, a]

        assert_eq!(q.current().map(|e| e.id), Some(b));
        assert_eq!(q.previous(), Some(a));
        assert_eq!(q.current().map(|e| e.id), Some(a));
    }

    #[test]
    fn view_context_builds_queue_with_selected_current() {
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let vc = ViewContext {
            songs: vec![s1, s2, s3],
            selected_index: 1,
        };
        let q = vc.build_queue();
        assert_eq!(q.len(), 3);
        assert_eq!(q.current().unwrap().item.song_id(), Some(s2));
        // Pending are the songs after the selection, in order.
        assert_eq!(q.pending_ids().len(), 1);
        let pending = q.pending();
        assert_eq!(pending[0].item.song_id(), Some(s3));
    }

    #[test]
    fn selected_index_clamped() {
        let s1 = song();
        let s2 = song();
        let vc = ViewContext {
            songs: vec![s1, s2],
            selected_index: 999,
        };
        let q = vc.build_queue();
        // Clamped to the last song.
        assert_eq!(q.current().unwrap().item.song_id(), Some(s2));
    }

    #[test]
    fn empty_view_yields_empty_queue() {
        let vc = ViewContext {
            songs: vec![],
            selected_index: 0,
        };
        let q = vc.build_queue();
        assert!(q.is_empty());
        assert!(q.current().is_none());
    }

    // -- Task 8.6: shuffle bag and mode-aware advance ------------------------

    #[test]
    fn shuffle_bag_pops_in_selected_order_once_each() {
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s1));
        let b = q.push(lib(s2));
        let c = q.push(lib(s3));
        q.set_current(a);
        // Shuffle order: play c, then a, then b (all three, one round).
        q.set_shuffle(true, vec![c, a, b]);
        assert_eq!(q.advance_in_mode(PlayMode::Shuffle), Some(c));
        assert_eq!(q.current().map(|e| e.id), Some(c));
        assert_eq!(q.advance_in_mode(PlayMode::Shuffle), Some(a));
        assert_eq!(q.advance_in_mode(PlayMode::Shuffle), Some(b));
        // Round exhausted.
        assert_eq!(q.advance_in_mode(PlayMode::Shuffle), None);
    }

    #[test]
    fn shuffle_bag_never_repeats_within_a_round() {
        let s = song(); // same song, three duplicates
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s));
        let b = q.push(lib(s1));
        let c = q.push(lib(s2));
        q.set_current(a);
        // A round of three distinct entry ids.
        q.set_shuffle(true, vec![b, c, a]);
        let mut seen = vec![];
        while let Some(id) = q.advance_in_mode(PlayMode::Shuffle) {
            seen.push(id);
        }
        // Duplicate SongId entries (a and b differ by id even if same song)
        // never fold together — a round visits each entry id once.
        assert_eq!(seen.len(), 3);
        for w in seen.windows(2) {
            assert_ne!(w[0], w[1]);
        }
    }

    #[test]
    fn sequential_mode_ignores_shuffle_bag() {
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s1));
        let b = q.push(lib(s2));
        q.set_current(a);
        q.set_shuffle(true, vec![b]); // bag would reorder, but sequential ignores it
        assert_eq!(q.advance_in_mode(PlayMode::Sequential), Some(b));
        // 列表循环: past the tail the queue wraps to the first entry.
        assert_eq!(q.advance_in_mode(PlayMode::Sequential), Some(a));
    }

    #[test]
    fn sequential_wraps_around_to_the_first_entry() {
        // 列表循环 (设计: 默认循环): after the last entry, playback returns to
        // the first one instead of stopping.
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s1));
        let b = q.push(lib(s2));
        let c = q.push(lib(s3));
        q.set_current(a);
        assert_eq!(q.advance_in_mode(PlayMode::Sequential), Some(b));
        assert_eq!(q.advance_in_mode(PlayMode::Sequential), Some(c));
        // Tail reached: wrap to the head, and the tail is pending again.
        assert_eq!(q.advance_in_mode(PlayMode::Sequential), Some(a));
        assert_eq!(q.pending_ids(), vec![b, c]);
        // A single-entry queue loops on itself.
        let mut solo = Queue::new();
        let only = solo.push(lib(s1));
        solo.set_current(only);
        assert_eq!(solo.advance_in_mode(PlayMode::Sequential), Some(only));
    }

    #[test]
    fn repeat_one_returns_current_forever() {
        let s1 = song();
        let s2 = song();
        let mut q = Queue::new();
        let a = q.push(lib(s1));
        let _b = q.push(lib(s2));
        q.set_current(a);
        // Repeat-one ignores pending and always returns the current id.
        assert_eq!(q.advance_in_mode(PlayMode::RepeatOne), Some(a));
        assert_eq!(q.advance_in_mode(PlayMode::RepeatOne), Some(a));
    }
}
