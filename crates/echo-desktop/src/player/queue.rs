//! Playback queue, history and the coordinator rules (task 8.5, 8.6, 8.7).
//!
//! The queue lives entirely on the desktop player layer (design §12: 队列与播放
//! 会话留在桌面层). [`QueueEntryId`] from `echo-core` is the stable identity of
//! one *entry*; the same [`SongId`] appended twice gets two independent entry
//! IDs and is never collapsed (design §12: 同一 `SongId` 的每次追加拥有不同
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
//!
//! **Module layout** — the aggregate is split along the three concerns it
//! mixes, so no single file has to carry all of them:
//! - `item`: what a queue entry refers to ([`QueueEntry`], [`QueueItem`], the
//!   session-only temporary item and its metadata/lyrics).
//! - `revision`: the monotonic [`QueueRevision`] the transport uses to decide
//!   whether the projection must be shipped again.
//! - `ordering`: the playback order — the user-facing projection, advancing
//!   per play mode, and the history bookkeeping.
//! - `mutation`: the content changes — joining, the "play next" lane, shuffle
//!   rounds, blocked entries, clearing and removal.
//!
//! This module holds the aggregate itself: the state, the read-only accessors
//! and the entry point points that stay stable for the rest of the crate.

use std::time::{SystemTime, UNIX_EPOCH};

use echo_core::domain::ids::{QueueEntryId, SongId};

mod item;
mod mutation;
mod ordering;
mod revision;

pub use item::{
    HistoryRecord, QueueEntry, QueueItem, TemporaryItem, TemporaryLyricLine, TemporaryLyrics,
    TemporaryMetadata,
};
pub use revision::QueueRevision;

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
    /// Recently played entries, newest first. Retention is time based so
    /// previous remains meaningful across restart without unbounded growth.
    history: Vec<HistoryRecord>,
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
    /// Entries explicitly requested through "play next", in the exact FIFO
    /// order in which they must be consumed.  The entries also live in
    /// `entries`; this separate ID-only lane preserves the user's intent even
    /// when a circular projection crosses the physical storage boundary.
    priority: Vec<QueueEntryId>,
    /// Entries restored while their media/root is unavailable. They remain in
    /// the queue and UI projection, but traversal skips them until retried.
    blocked: std::collections::HashSet<QueueEntryId>,
    /// Monotonic revision of the projection this queue renders.
    revision: QueueRevision,
}

/// A result of advancing/removing that the caller should surface.
#[derive(Clone, Debug, PartialEq, Eq)]
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
            shuffle_bag: Vec::new(),
            shuffle_active: false,
            priority: Vec::new(),
            blocked: std::collections::HashSet::new(),
            revision: QueueRevision::INITIAL,
        }
    }

    /// The current projection revision. The transport compares it with the
    /// revision it last delivered to decide whether the projection must be
    /// shipped again.
    #[must_use]
    pub const fn revision(&self) -> QueueRevision {
        self.revision
    }

    /// Advance the revision without changing the queue itself.
    ///
    /// The coordinator uses it for the projection state it owns outside the
    /// queue — the failed-round set and the playback mode — which is part of
    /// the projection the UI renders.
    pub const fn bump_revision(&mut self) {
        self.revision = self.revision.next();
    }

    /// Continue this queue's revision from `revision` and advance it once.
    ///
    /// Used when the whole queue is replaced: the replacement is itself a
    /// projection change, so the new queue must continue strictly *after* the
    /// old revision instead of restarting the counter.
    pub const fn continue_revision(&mut self, revision: QueueRevision) {
        self.revision = revision.next();
    }

    /// Whether shuffle mode is active (the bag is consulted on advance).
    #[must_use]
    pub const fn is_shuffle(&self) -> bool {
        self.shuffle_active
    }

    /// The current shuffle bag (ids remaining in this round, in play order).
    #[must_use]
    pub fn shuffle_bag(&self) -> &[QueueEntryId] {
        &self.shuffle_bag
    }

    /// The manual "play next" lane, in actual consumption order.
    #[must_use]
    pub fn priority_ids(&self) -> &[QueueEntryId] {
        &self.priority
    }

    /// The number of entries (current + pending; history entries that were
    /// removed are not counted).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the queue is empty (no current and no pending).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
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
    pub fn history(&self) -> Vec<QueueEntryId> {
        self.history.iter().map(|record| record.entry_id).collect()
    }

    /// Timestamped history records, newest first, used for durable session
    /// persistence. Entries older than three days are pruned before exposure.
    #[must_use]
    pub fn history_records(&self) -> &[HistoryRecord] {
        &self.history
    }

    /// Look up an entry by id (covers current, pending and history entries
    /// that are still in the `entries` list).
    #[must_use]
    pub fn get(&self, id: QueueEntryId) -> Option<&QueueEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Mutable lookup by stable queue-entry identity. The coordinator uses
    /// this for in-place identity transitions that must not reorder history or
    /// the pending lanes.
    pub fn get_mut(&mut self, id: QueueEntryId) -> Option<&mut QueueEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    #[must_use]
    pub fn is_blocked(&self, entry_id: QueueEntryId) -> bool {
        self.blocked.contains(&entry_id)
    }

    /// The ids of the entries that are currently unavailable.
    ///
    /// They stay in the projection (the user must see *why* an item did not
    /// play) but are skipped by traversal until their availability returns.
    #[must_use]
    pub fn blocked_ids(&self) -> Vec<QueueEntryId> {
        self.blocked.iter().copied().collect()
    }

    /// All entries in append order — current, pending, and any historical
    /// entry still present in the list. Used by session persistence (8.9) to
    /// snapshot the full queue with independent entry ids intact.
    #[must_use]
    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    /// Whether an entry (by id) is currently pending (not current).
    #[must_use]
    pub fn is_pending(&self, entry_id: QueueEntryId) -> bool {
        self.current_index.is_some_and(|index| {
            self.entries[index + 1..]
                .iter()
                .any(|entry| entry.id == entry_id)
        })
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

const HISTORY_RETENTION_MS: u64 = 3 * 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn history_expired(played_at_ms: u64) -> bool {
    now_ms().saturating_sub(played_at_ms) > HISTORY_RETENTION_MS
}

/// A view-context play request: the coordinator derives the queue from a
/// deterministic list of songs (server-resolved by the Core playback-context
/// request) starting at a selected song. Task 8.5: 视图上下文.
#[derive(Clone, Debug, PartialEq, Eq)]
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
mod tests;
