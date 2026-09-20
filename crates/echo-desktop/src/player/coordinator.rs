//! `PlaybackCoordinator` — drives the queue against a `PlayerPort` (task 8.5).
//!
//! The coordinator is the single owner of "what should play next": it holds a
//! [`Queue`], decides the next entry per the active [`super::port::PlayMode`],
//! and issues `LoadLibrarySong` / `LoadTemporary` commands to the player port.
//! It also handles automatic skip on playback failure (`BackendEvent::Ended`
//! due to error), so an unplayable entry does not stall the queue.
//!
//! Task 8.5 scope:
//! - view context (build a queue from a view, selected song current);
//! - current / history navigation;
//! - join queue (append), next-track insertion, clear pending;
//! - error skip on a failed load (auto-advance, never spin).
//!
//! Duplicate `SongId`s always appear as independent `QueueEntryId`s (design
//! §12). No I/O here: the coordinator delegates file resolution to the
//! platform layer and just answers "what is current / what is next".

use echo_core::domain::ids::{PlaybackSessionId, QueueEntryId};

use super::port::{PlayMode, PlayerCommand, PlayerPort, PlayerSnapshot};
use super::queue::{Queue, QueueEntry, QueueItem, ViewContext};

/// "Previous" restarts the current track when it has played longer than this
/// (task 8.6: ">5 秒上一首回到开头").
const PREVIOUS_RESTART_THRESHOLD: f64 = 5.0;

/// The source of a transport transition. Keeping these distinct makes the
/// repeat-one and manual-priority contracts explicit instead of encoding them
/// as temporary mode switches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NavigationEvent {
    NaturalEnd,
    ManualNext,
    ManualPrevious,
    ErrorSkip,
}

/// Drives a [`Queue`] over a [`PlayerPort`].
pub struct PlaybackCoordinator<P: PlayerPort, S: ShuffleSource = DefaultShuffle> {
    player: P,
    queue: Queue,
    /// Active playback mode (sequential / shuffle / repeat-one).
    mode: PlayMode,
    /// The `PlaybackSessionId` of the current load, for statistics (8.10).
    active_load_session: Option<(QueueEntryId, PlaybackSessionId)>,
    /// Randomness source for building shuffle rounds. Tests inject a
    /// deterministic permutation.
    shuffle: S,
    /// The set of entries that failed to load/decode in the *current round*,
    /// keyed by `queueEntryId` (task 8.7). Each entry is auto-attempted at
    /// most once per round; error advance skips these rather than retrying
    /// (and bypasses repeat-one). Cleared when a fresh queue session starts
    /// (`play_context` / `play_temporary`) so a damaged file is retried on the
    /// next explicit play.
    failed_round: std::collections::HashSet<QueueEntryId>,
}

/// A pluggable source of randomness for shuffle-round construction. The
/// default is a lightweight deterministic LCG seeded per round (so a round is
/// reproducible given the same pending ids and seed); tests supply a fixed
/// permutation to assert exact play order.
pub trait ShuffleSource: Send {
    /// Return a permutation of `0..len` — the order pending entries play in a
    /// round. `len == 0` yields an empty permutation.
    fn shuffle(&mut self, len: usize, seed: u64) -> Vec<usize>;
}

/// Default [`ShuffleSource`]: a Fisher–Yates shuffle over an LCG.
#[derive(Default)]
pub struct DefaultShuffle;

impl ShuffleSource for DefaultShuffle {
    fn shuffle(&mut self, len: usize, seed: u64) -> Vec<usize> {
        let mut xs: Vec<usize> = (0..len).collect();
        let mut state: u64 = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        for i in (1..len).rev() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let j = (state >> 33) as usize % (i + 1);
            xs.swap(i, j);
        }
        xs
    }
}

impl<P: PlayerPort> PlaybackCoordinator<P> {
    /// A new coordinator bound to a player port and the default shuffle source.
    #[must_use]
    pub fn new(player: P) -> Self {
        Self::with_shuffle(player, DefaultShuffle)
    }
}

impl<P: PlayerPort, S: ShuffleSource> PlaybackCoordinator<P, S> {
    /// A new coordinator with a custom shuffle source (for deterministic
    /// tests). Playback behavior is otherwise identical.
    #[must_use]
    pub fn with_shuffle(player: P, shuffle: S) -> Self {
        Self {
            player,
            queue: Queue::new(),
            mode: PlayMode::Sequential,
            active_load_session: None,
            shuffle,
            failed_round: std::collections::HashSet::new(),
        }
    }

    /// The current snapshot from the player port.
    #[must_use]
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.player.snapshot()
    }

    /// The active playback mode.
    #[must_use]
    pub const fn mode(&self) -> PlayMode {
        self.mode
    }

    /// Immutable access to the queue.
    #[must_use]
    pub const fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The authoritative user-facing queue order: the current entry first,
    /// followed by mode-aware pending entries.  The queue storage itself stays
    /// private to traversal, history and persistence concerns.
    #[must_use]
    pub fn queue_view(&self) -> Vec<QueueEntry> {
        self.queue.view_entries()
    }

    /// Mutable access to the queue — for the deletion coordinator (task 8.11),
    /// which snapshots/removes entries under the same lock the command layer
    /// already holds. Every other mutation goes through coordinator methods.
    #[must_use]
    pub fn queue_mut(&mut self) -> &mut Queue {
        &mut self.queue
    }

    /// The current queue entry, if any.
    #[must_use]
    pub fn current(&self) -> Option<&QueueEntry> {
        self.queue.current()
    }

    /// The pending queue entries (up-next).
    #[must_use]
    pub fn pending(&self) -> Vec<&QueueEntry> {
        self.queue.pending()
    }

    /// Start playback of a view context: build the queue, set the selected
    /// song current, and load it immediately.
    /// # Panics
    ///
    /// Panics only if queue construction violates the `ViewContext` invariant
    /// that the chosen song becomes the current queue entry.
    pub fn play_context(&mut self, ctx: &ViewContext) {
        // The coordinator derives the queue from the server-resolved songs; the
        // selected song becomes current.
        let selected = ctx.selected_index.min(ctx.songs.len().saturating_sub(1));
        let song = ctx.songs[selected];
        self.queue = ctx.build_queue();
        self.failed_round.clear(); // fresh session: retry previously-failed files
        let current_id = self
            .queue
            .current_id()
            .expect("view context always yields a current entry");
        self.load_entry(current_id, song);
        // The user's mode persists across context switches (设计: 记忆播放模式) —
        // a new context never resets it.
    }

    /// Cold-start priming (设计: 有歌时默认入栏，不发声): build the queue from
    /// the view context exactly like [`Self::play_context`], but load the
    /// selected song **paused** — the file is decoded so the player bar shows
    /// real metadata/duration, and no sound is ever produced until the user
    /// presses play.
    pub fn play_context_paused(&mut self, ctx: &ViewContext) {
        let selected = ctx.selected_index.min(ctx.songs.len().saturating_sub(1));
        let song = ctx.songs[selected];
        self.queue = ctx.build_queue();
        self.failed_round.clear();
        if let Some(current_id) = self.queue.current_id() {
            let session = self.new_load_session(current_id);
            self.player
                .send(PlayerCommand::LoadLibrarySongPaused {
                    song_id: song,
                    session_id: session,
                })
                .ok();
        }
    }

    /// Prime a new context while preserving restored playback settings. This
    /// is used when a persisted current entry is gone or after the first
    /// successful library import; it must remain paused and must not be
    /// treated as a user-initiated play action.
    pub fn prime_context_paused(
        &mut self,
        ctx: &ViewContext,
        mode: PlayMode,
        volume: f64,
        muted: bool,
    ) {
        self.mode = mode;
        self.player.send(PlayerCommand::SetMode(mode)).ok();
        self.player.send(PlayerCommand::SetVolume(volume)).ok();
        self.player.send(PlayerCommand::SetMute(muted)).ok();
        self.play_context_paused(ctx);
    }

    /// Apply a restored playback session (task 8.9, 冷启动恢复): adopt the
    /// rebuilt queue and mode, restore the volume/mute settings and last valid
    /// position on the actor, and load the current entry **paused** (恢复后绝不
    /// 自动发声). Commands are ordered by the actor, so the seek follows the
    /// paused load rather than racing it from another thread.
    pub fn restore_session(
        &mut self,
        queue: Queue,
        mode: PlayMode,
        volume: f64,
        muted: bool,
        position: Option<f64>,
    ) {
        self.queue = queue;
        self.mode = mode;
        self.failed_round.clear();
        self.player.send(PlayerCommand::SetVolume(volume)).ok();
        // `SetMute`, not `ToggleMute`: the session records the state the user
        // left behind, so restoring must reproduce it. A relative toggle is
        // only correct if this runs exactly once — a repeated restore (a
        // double-invoked command, React StrictMode mounting twice) would flip
        // the flag back and silently un-mute the player. The absolute form is
        // idempotent.
        self.player.send(PlayerCommand::SetMute(muted)).ok();
        if self
            .queue
            .current_id()
            .is_some_and(|id| self.queue.is_blocked(id))
        {
            // A restored unavailable current remains visible in the projection,
            // but must never be sent to the actor. Advance to a playable entry
            // without deleting or reordering the blocked record.
            self.next_from_mode(self.mode);
        } else if let Some(id) = self.queue.current_id() {
            if let Some(song) = self.queue.get(id).and_then(|e| e.item.song_id()) {
                let session = self.new_load_session(id);
                self.player
                    .send(PlayerCommand::LoadLibrarySongPaused {
                        song_id: song,
                        session_id: session,
                    })
                    .ok();
                if let Some(position) = position.filter(|value| value.is_finite() && *value >= 0.0)
                {
                    self.player.send(PlayerCommand::Seek(position)).ok();
                }
            }
        }
    }

    /// Play a single temporary file immediately (system file open outside the
    /// active library). The temporary item becomes the sole current entry.
    pub fn play_temporary(&mut self, item: TemporaryPlay) {
        let display = item.display_name.clone();
        let path = item.path.clone();
        let entry = QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Temporary(super::queue::TemporaryItem {
                display_name: item.display_name,
                path: item.path,
                duration: item.duration,
                on_active_root: item.on_active_root,
            }),
        };
        let id = entry.id;
        self.queue = Queue::new();
        self.queue.push(entry);
        self.queue.set_current(id);
        self.failed_round.clear(); // fresh session
        let session = self.new_load_session(id);
        self.player
            .send(PlayerCommand::LoadTemporary {
                display_name: display,
                path,
                session_id: session,
            })
            .ok();
    }

    /// Append an entry to the end of the queue ("加入队列", 8.5).
    /// Returns the new entry id.
    pub fn enqueue(&mut self, entry: QueueEntry) -> QueueEntryId {
        self.queue.push(entry)
    }

    /// Insert an entry immediately after the current ("下一首播放"). Returns
    /// the new entry id.
    pub fn play_next(&mut self, entry: QueueEntry) -> QueueEntryId {
        self.queue.insert_next(entry)
    }

    /// Clear all pending entries, keeping the current one playing.
    pub fn clear_pending(&mut self) {
        self.queue.clear_pending();
    }

    /// Switch the playback mode (task 8.6).
    ///
    /// Entering shuffle builds a fresh round from the current *pending* ids,
    /// seeded by the coordinate position so a repeated switch produces a
    /// reproducible round for identical pending sets; the current item is
    /// never disturbed. Leaving shuffle keeps the bag as-is so re-entering
    /// resumes where it left off (恢复后不重洗) — the coordinator persists the
    /// bag externally in task 8.9.
    pub fn set_mode(&mut self, mode: PlayMode) {
        if mode == PlayMode::Shuffle {
            self.enter_shuffle();
        } else {
            // Leaving shuffle preserves the bag; Sequential/RepeatOne just
            // stop consulting it. The current entry is unchanged.
            self.queue
                .set_shuffle(false, self.queue.shuffle_bag().to_vec());
        }
        self.mode = mode;
        // Mode is coordinator-owned queue state, but the UI is driven only by
        // the player snapshot stream. Publish this discrete change through the
        // port so switching modes while paused or with an empty queue updates
        // the visible control immediately.
        self.player.send(PlayerCommand::SetMode(mode)).ok();
    }

    fn enter_shuffle(&mut self) {
        let pending_ids = self.queue.pending_ids();
        let len = pending_ids.len();
        let order = self.shuffle.shuffle(len, self.round_seed());
        let bag: Vec<QueueEntryId> = order.into_iter().map(|i| pending_ids[i]).collect();
        self.queue.set_shuffle(true, bag);
    }

    /// A seed that changes when the pending set changes, so entering shuffle
    /// twice without a queue change gives the same round (deterministic), but
    /// a changed pending set re-seeds. Derive it from the length + first id so
    /// it is reproducible.
    fn round_seed(&self) -> u64 {
        let len = self.queue.pending_ids().len() as u64;
        let first = self
            .queue
            .pending_ids()
            .first()
            .map_or(0, |id| Self::id_key(*id));
        len.wrapping_mul(2_654_435_761).wrapping_add(first)
    }

    fn id_key(id: QueueEntryId) -> u64 {
        // Use the entry's raw bits when available; fall back to its position
        // hash. QueueEntryId is an opaque 96-bit id — fold to u64.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        h.finish()
    }

    /// Go to the next entry (user presses "next") per the current mode. An
    /// explicit next always advances — even in repeat-one, the user's "next"
    /// leaves the current entry (design §8.6: 切模式不丢当前项, explicit next
    /// 离开单曲循环). In shuffle mode the shuffle bag supplies the order; when
    /// the bag empties it is refreshed for a fresh round. Returns the new
    /// current entry id, or `None` if the queue stopped.
    pub fn advance_to_next(&mut self) -> Option<QueueEntryId> {
        // A one-entry view has no different next item. Re-loading the same
        // file creates an unnecessary Loading -> Paused window in mpv and can
        // leave the UI transport state behind the audible player. Treat both
        // directions as an explicit replay instead.
        if self.queue.len() == 1 && self.replay_current() {
            return self.queue.current_id();
        }
        self.navigate(NavigationEvent::ManualNext)
    }

    fn navigate(&mut self, event: NavigationEvent) -> Option<QueueEntryId> {
        match event {
            NavigationEvent::ManualPrevious => {
                return self.previous_from_mode();
            }
            NavigationEvent::NaturalEnd if self.mode == PlayMode::RepeatOne => {
                let id = self.queue.current_id()?;
                let song = self.queue.get(id).and_then(|entry| entry.item.song_id())?;
                self.load_entry(id, song);
                return Some(id);
            }
            NavigationEvent::NaturalEnd | NavigationEvent::ManualNext => {
                if let Some(id) = self.queue.advance_priority() {
                    return self.load_queue_entry(id);
                }
            }
            NavigationEvent::ErrorSkip => {}
        }

        // A manual next in repeat-one follows list-loop order, but does not
        // silently change the user-selected mode. Error skip uses the same
        // transient order to avoid retrying a failed repeat-one entry.
        let mode = if matches!(
            event,
            NavigationEvent::ManualNext | NavigationEvent::ErrorSkip
        ) && self.mode == PlayMode::RepeatOne
        {
            PlayMode::Sequential
        } else {
            self.mode
        };
        self.next_from_mode(mode)
    }

    fn next_from_mode(&mut self, mode: PlayMode) -> Option<QueueEntryId> {
        // 列表循环 makes sequential advance wrap forever, so "no progress"
        // (every entry failed this round) can no longer be detected by an
        // `Exhausted` return. Cap the walk at one full loop plus the wrap; a
        // cap out means nothing playable remains — stop instead of spinning.
        let max_attempts = self.queue.len() + 1;
        for _ in 0..max_attempts {
            if let Some(id) = self.queue.advance_in_mode(mode) {
                if self.failed_round.contains(&id) || self.queue.is_blocked(id) {
                    // This entry failed earlier this round — auto-attempt
                    // it only once; skip it (task 8.7).
                    continue;
                }
                return self.load_queue_entry(id);
            }
            if mode == PlayMode::Shuffle {
                // Bag exhausted: refresh a fresh round from pending.
                let pending_ids = self.queue.pending_ids();
                // If every pending entry already failed this round,
                // stop rather than spin (task 8.7).
                let all_failed = !pending_ids.is_empty()
                    && pending_ids.iter().all(|id| self.failed_round.contains(id));
                if pending_ids.is_empty() || all_failed {
                    self.player.send(PlayerCommand::Stop).ok();
                    return None;
                }
                let order = self.shuffle.shuffle(pending_ids.len(), self.round_seed());
                let bag: Vec<QueueEntryId> = order.into_iter().map(|i| pending_ids[i]).collect();
                self.queue.set_shuffle(true, bag);
                continue;
            }
            // No pending (or all sequential pending were failed/skipped):
            // stop playing.
            self.player.send(PlayerCommand::Stop).ok();
            return None;
        }
        // A full loop found nothing un-failed: stop (never spin).
        self.player.send(PlayerCommand::Stop).ok();
        None
    }

    /// Record that the current entry failed to load/decode and advance past it
    /// per the error-skip rules (task 8.7). The failed entry is added to the
    /// round's error set, bypasses repeat-one, and is never auto-retried in
    /// this round. Returns the new current entry id, or `None` when the queue
    /// stopped (all remaining entries failed or none pending).
    pub fn on_load_error(&mut self) -> Option<QueueEntryId> {
        if let Some(id) = self.queue.current_id() {
            self.failed_round.insert(id);
        }
        self.navigate(NavigationEvent::ErrorSkip)
    }

    /// The set of entries that failed in the current round (for diagnostics).
    pub fn failed_round(&self) -> impl Iterator<Item = QueueEntryId> + '_ {
        self.failed_round.iter().copied()
    }

    /// Go to the previous entry. If the current track has played for more than
    /// 5 seconds, "previous" restarts the current track (seek to 0) instead of
    /// moving back (task 8.6: ">5 秒上一首回到开头"). Otherwise it moves to
    /// the previous entry in history.
    pub fn previous(&mut self) {
        // There is no historical/list predecessor in a one-entry queue. A
        // seek plus an explicit play keeps the replay audible and makes the
        // transport state authoritative immediately.
        if self.queue.len() == 1 && self.replay_current() {
            return;
        }
        let now = self.player.snapshot().position.unwrap_or(0.0);
        // "Is anything loaded" is a queue fact, so ask the queue. The player
        // snapshot carries transport state only (it has no queue-entry id), and
        // reading it here made the >5 s restart rule dead code in production
        // while the FakePlayer-based test stayed green.
        let has_current = self.queue.current().is_some();
        if has_current && now > PREVIOUS_RESTART_THRESHOLD {
            // Restart the current track.
            self.player.send(PlayerCommand::Seek(0.0)).ok();
            return;
        }
        let _ = self.navigate(NavigationEvent::ManualPrevious);
    }

    /// Seek to an absolute position (seconds) within the current track.
    ///
    /// Task 8.8: the command is forwarded to the player port, which owns the
    /// authoritative rollback — a rejected backend write leaves the snapshot
    /// (and therefore the UI) untouched, so the queue never shows a position
    /// mpv did not reach.
    pub fn seek(&mut self, position: f64) {
        self.player.send(PlayerCommand::Seek(position)).ok();
    }

    /// Set the output volume (0.0–1.0). The player port clamps and owns the
    /// authoritative rollback: a rejected write keeps the previous volume so
    /// the UI (driven by the snapshot) is never shown a value mpv rejected.
    pub fn set_volume(&mut self, volume: f64) {
        self.player.send(PlayerCommand::SetVolume(volume)).ok();
    }

    /// Toggle mute, remembering the last non-zero volume so unmute restores it.
    /// The player port owns the rollback (task 8.8).
    pub fn toggle_mute(&mut self) {
        self.player.send(PlayerCommand::ToggleMute).ok();
    }

    /// React to a playback state change: when a track ends (natural EOF or a
    /// failed load surfaced as `Ended`), auto-advance per the mode's rules,
    /// except in repeat-one where the current entry is repeated. Returns the
    /// new current entry id, or `None` if the queue stopped. (Error-specific
    /// skip is task 8.7.)
    pub fn on_played_to_end(&mut self) -> Option<QueueEntryId> {
        self.navigate(NavigationEvent::NaturalEnd)
    }

    /// Whether a queue entry exists (by id).
    #[must_use]
    pub fn contains(&self, id: QueueEntryId) -> bool {
        self.queue.contains(id)
    }

    /// Recheck restored blocked entries after a root/file recovery. Entries
    /// retain their identities and order; newly available items are eligible
    /// for the next transport action immediately.
    pub fn retry_blocked(
        &mut self,
        mut playable: impl FnMut(echo_core::domain::ids::SongId) -> bool,
    ) {
        let ids: Vec<_> = self
            .queue
            .entries()
            .iter()
            .filter_map(|entry| {
                self.queue
                    .is_blocked(entry.id)
                    .then(|| {
                        entry
                            .item
                            .song_id()
                            .filter(|song| playable(*song))
                            .map(|_| entry.id)
                    })
                    .flatten()
            })
            .collect();
        for id in ids {
            self.queue.set_blocked(id, false);
        }
    }

    /// Select an existing playable queue entry and start it immediately.
    ///
    /// The queue projection exposes entry IDs rather than song IDs because the
    /// same song may appear more than once. Selecting by entry ID preserves
    /// that identity, history and the current queue context.
    pub fn play_queue_entry(&mut self, entry_id: QueueEntryId) -> Option<QueueEntryId> {
        if self.queue.is_blocked(entry_id) || self.failed_round.contains(&entry_id) {
            return None;
        }
        let entry = self.queue.get(entry_id)?.clone();
        self.queue.set_current(entry_id)?;
        let session = self.new_load_session(entry_id);
        match entry.item {
            QueueItem::Library(song_id) => self
                .player
                .send(PlayerCommand::LoadLibrarySong {
                    song_id,
                    session_id: session,
                })
                .ok(),
            QueueItem::Temporary(item) => self
                .player
                .send(PlayerCommand::LoadTemporary {
                    display_name: item.display_name,
                    path: item.path,
                    session_id: session,
                })
                .ok(),
        };
        Some(entry_id)
    }

    fn new_load_session(&mut self, entry_id: QueueEntryId) -> PlaybackSessionId {
        let session = PlaybackSessionId::new();
        self.active_load_session = Some((entry_id, session));
        session
    }

    /// The active load session: the current entry id and the
    /// `PlaybackSessionId` issued for it (statistics, 8.10). `None` before the
    /// first load.
    #[must_use]
    pub const fn active_load_session(&self) -> Option<(QueueEntryId, PlaybackSessionId)> {
        self.active_load_session
    }

    /// Access the player port directly (for tests that need to inspect the
    /// fake player's internal state, e.g. `last_loaded_song`).
    #[must_use]
    pub const fn player(&self) -> &P {
        &self.player
    }

    fn load_entry(&mut self, entry_id: QueueEntryId, song: echo_core::domain::ids::SongId) {
        let session = self.new_load_session(entry_id);
        self.player
            .send(PlayerCommand::LoadLibrarySong {
                song_id: song,
                session_id: session,
            })
            .ok();
    }

    fn load_queue_entry(&mut self, entry_id: QueueEntryId) -> Option<QueueEntryId> {
        let song = self
            .queue
            .get(entry_id)
            .and_then(|entry| entry.item.song_id())?;
        self.load_entry(entry_id, song);
        Some(entry_id)
    }

    /// Restart the sole current queue entry from the beginning.
    ///
    /// Returns `false` when the queue has no current entry, allowing callers
    /// to retain their normal navigation fallback.
    fn replay_current(&self) -> bool {
        if self.queue.current_id().is_none() {
            return false;
        }
        self.player.send(PlayerCommand::Seek(0.0)).ok();
        self.player.send(PlayerCommand::Play).ok();
        true
    }

    fn previous_from_mode(&mut self) -> Option<QueueEntryId> {
        let previous = if self.mode == PlayMode::Sequential {
            self.queue.previous_in_loop()
        } else {
            self.queue.previous()
        }?;
        self.load_queue_entry(previous)
    }
}

/// A request to play a temporary file (from the platform open-file boundary).
#[derive(Clone, Debug)]
pub struct TemporaryPlay {
    pub display_name: String,
    pub path: std::path::PathBuf,
    pub duration: Option<f64>,
    pub on_active_root: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests;
