//! PlaybackCoordinator — drives the queue against a `PlayerPort` (task 8.5).
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
    /// (play_context / play_temporary) so a damaged file is retried on the
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
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for i in (1..len).rev() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = (state >> 33) as usize % (i + 1);
            xs.swap(i, j);
        }
        xs
    }
}

impl<P: PlayerPort> PlaybackCoordinator<P> {
    /// A new coordinator bound to a player port and the default shuffle source.
    #[must_use]
    pub fn new(player: P) -> PlaybackCoordinator<P, DefaultShuffle> {
        PlaybackCoordinator::with_shuffle(player, DefaultShuffle)
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

    /// Apply a restored playback session (task 8.9, 冷启动恢复): adopt the
    /// rebuilt queue and mode, restore the volume/mute settings on the actor,
    /// and load the current entry **paused** (恢复后绝不自动发声). The current
    /// entry's last position is persisted but intentionally not seeked here —
    /// the load completes asynchronously; resuming mid-track is a follow-up.
    pub fn restore_session(&mut self, queue: Queue, mode: PlayMode, volume: f64, muted: bool) {
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
        if let Some(id) = self.queue.current_id() {
            if let Some(song) = self.queue.get(id).and_then(|e| e.item.song_id()) {
                let session = self.new_load_session(id);
                self.player
                    .send(PlayerCommand::LoadLibrarySongPaused {
                        song_id: song,
                        session_id: session,
                    })
                    .ok();
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
            .map(|id| self.id_key(*id))
            .unwrap_or(0);
        len.wrapping_mul(2654435761).wrapping_add(first)
    }

    fn id_key(&self, id: QueueEntryId) -> u64 {
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
        // Explicit next always leaves repeat-one.
        if self.mode == PlayMode::RepeatOne {
            self.mode = match self.queue.is_shuffle() {
                true => PlayMode::Shuffle,
                false => PlayMode::Sequential,
            };
        }
        self.next_from_mode()
    }

    fn next_from_mode(&mut self) -> Option<QueueEntryId> {
        // 列表循环 makes sequential advance wrap forever, so "no progress"
        // (every entry failed this round) can no longer be detected by an
        // `Exhausted` return. Cap the walk at one full loop plus the wrap; a
        // cap out means nothing playable remains — stop instead of spinning.
        let max_attempts = self.queue.len() + 1;
        for _ in 0..max_attempts {
            match self.queue.advance_in_mode(self.mode) {
                Some(id) => {
                    if self.failed_round.contains(&id) {
                        // This entry failed earlier this round — auto-attempt
                        // it only once; skip it (task 8.7).
                        continue;
                    }
                    if let Some(song) = self.queue.get(id).and_then(|e| e.item.song_id()) {
                        self.load_entry(id, song);
                    }
                    return Some(id);
                }
                None => {
                    if self.mode == PlayMode::Shuffle {
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
                        let bag: Vec<QueueEntryId> =
                            order.into_iter().map(|i| pending_ids[i]).collect();
                        self.queue.set_shuffle(true, bag);
                        continue;
                    }
                    // No pending (or all sequential pending were failed/skipped):
                    // stop playing.
                    self.player.send(PlayerCommand::Stop).ok();
                    return None;
                }
            }
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
        // Error advance bypasses repeat-one (design §8.7: 错误推进绕过单曲循环).
        if self.mode == PlayMode::RepeatOne {
            self.mode = match self.queue.is_shuffle() {
                true => PlayMode::Shuffle,
                false => PlayMode::Sequential,
            };
        }
        self.next_from_mode()
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
        if let Some(prev_id) = self.queue.previous() {
            if let Some(song) = self.queue.get(prev_id).and_then(|e| e.item.song_id()) {
                self.load_entry(prev_id, song);
            }
        }
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
        if self.mode == PlayMode::RepeatOne {
            // Repeat the current entry.
            if let Some(id) = self.queue.current_id() {
                if let Some(song) = self.queue.get(id).and_then(|e| e.item.song_id()) {
                    self.load_entry(id, song);
                    return Some(id);
                }
            }
            return None;
        }
        self.next_from_mode()
    }

    /// Whether a queue entry exists (by id).
    #[must_use]
    pub fn contains(&self, id: QueueEntryId) -> bool {
        self.queue.contains(id)
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
    pub fn player(&self) -> &P {
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
mod tests {
    use super::*;
    use crate::player::fake::FakePlayer;
    use crate::player::queue::TemporaryItem;
    use echo_core::domain::ids::SongId;

    /// A deterministic [`ShuffleSource`] that returns a fixed permutation so
    /// shuffle play order is fully asserted in tests.
    #[derive(Clone)]
    struct FixedShuffle(Vec<usize>);

    impl FixedShuffle {
        fn from_order(order: Vec<usize>) -> Self {
            Self(order)
        }
    }

    impl ShuffleSource for FixedShuffle {
        fn shuffle(&mut self, len: usize, _seed: u64) -> Vec<usize> {
            if self.0.len() == len {
                std::mem::take(&mut self.0)
            } else {
                // Fallback: identity for unexpected lengths.
                (0..len).collect()
            }
        }
    }

    fn song() -> SongId {
        SongId::new()
    }

    fn lib_entry(s: SongId) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s),
        }
    }

    #[test]
    fn play_context_loads_selected_song() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        let s3 = song();
        let ctx = ViewContext {
            songs: vec![s1, s2, s3],
            selected_index: 1,
        };
        coord.play_context(&ctx);
        let last = coord.player().last_loaded_song();
        assert_eq!(last, Some(s2));
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        assert_eq!(coord.pending().len(), 1);
    }

    #[test]
    fn duplicate_song_can_appear_multiple_times_in_queue() {
        let s = song();
        let mut coord = PlaybackCoordinator::new(FakePlayer::new());
        let a = coord.enqueue(lib_entry(s));
        let b = coord.enqueue(lib_entry(s));
        assert_ne!(a, b);
        // Both distinct SongIds present as independent entries.
        assert_eq!(coord.queue().distinct_song_ids(), vec![s]);
        assert_eq!(coord.queue().len(), 2);
    }

    #[test]
    fn clear_pending_keeps_current_and_queue_of_one() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.enqueue(lib_entry(s1));
        coord.enqueue(lib_entry(s2));
        // Start playing s1.
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        coord.clear_pending();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        assert!(coord.pending().is_empty());
    }

    #[test]
    fn next_advances_in_order_and_wraps_around() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        coord.advance_to_next();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        // 列表循环: advancing past the end returns to the first entry.
        coord.advance_to_next();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        assert_eq!(
            coord.snapshot().state,
            echo_core::domain::state::PlaybackState::Playing,
            "the queue keeps playing after the wrap"
        );
    }

    #[test]
    fn play_context_keeps_the_user_mode() {
        // 设计: 记忆播放模式 — a new context never resets the mode back to
        // sequential (the old hardcode did exactly that on every play click).
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        coord.set_mode(PlayMode::RepeatOne);
        coord.play_context(&ViewContext {
            songs: vec![song(), song()],
            selected_index: 0,
        });
        assert_eq!(coord.mode(), PlayMode::RepeatOne);
    }

    #[test]
    fn play_context_paused_loads_without_playing() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        coord.play_context_paused(&ViewContext {
            songs: vec![s1, song()],
            selected_index: 0,
        });
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        assert_eq!(
            coord.snapshot().state,
            echo_core::domain::state::PlaybackState::Paused,
            "cold-start priming must never make a sound"
        );
        assert_eq!(coord.player().last_loaded_song(), Some(s1));
    }

    #[test]
    fn restore_session_recovers_queue_mode_and_settings_paused() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        let queue = ViewContext {
            songs: vec![s1, s2],
            selected_index: 1,
        }
        .build_queue();
        // current is s2 per the view context.
        assert_eq!(queue.current().unwrap().item.song_id(), Some(s2));
        coord.restore_session(queue, PlayMode::Shuffle, 0.42, true);
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        assert_eq!(coord.mode(), PlayMode::Shuffle);
        assert_eq!(
            coord.snapshot().state,
            echo_core::domain::state::PlaybackState::Paused
        );
        assert!((coord.snapshot().volume - 0.42).abs() < f64::EPSILON);
        assert!(coord.snapshot().muted);
    }

    #[test]
    fn restore_session_replay_keeps_persisted_mute() {
        // A restore that runs twice (a double-invoked boot command, React
        // StrictMode mounting the boot effect twice) must reproduce the
        // recorded state instead of toggling it away. The coordinator sends the
        // absolute `SetMute`, so the replay is a no-op; a relative toggle would
        // have un-muted the player on the second pass.
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let build = || {
            ViewContext {
                songs: vec![s1],
                selected_index: 0,
            }
            .build_queue()
        };
        coord.restore_session(build(), PlayMode::Sequential, 0.45, true);
        assert!(coord.snapshot().muted, "the first restore mutes");
        coord.restore_session(build(), PlayMode::Sequential, 0.45, true);
        assert!(coord.snapshot().muted, "replaying the restore stays muted");
        assert!((coord.snapshot().volume - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn error_end_auto_skips_to_next_available() {
        // When a track fails (Ended surfaced), the coordinator auto-advances
        // rather than stalling on the bad entry.
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        let advanced = coord.on_played_to_end();
        assert!(advanced.is_some());
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
    }

    #[test]
    fn play_temporary_is_session_only() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        coord.play_temporary(TemporaryPlay {
            display_name: "x.mp3".into(),
            path: std::path::PathBuf::from("/tmp/x.mp3"),
            duration: None,
            on_active_root: false,
        });
        assert_eq!(coord.current().unwrap().item.song_id(), None);
        assert_eq!(
            coord.current().unwrap().item,
            QueueItem::Temporary(TemporaryItem {
                display_name: "x.mp3".into(),
                path: std::path::PathBuf::from("/tmp/x.mp3"),
                duration: None,
                on_active_root: false,
            })
        );
    }

    // -- Task 8.6: modes, shuffle bag, ">5s previous" ------------------------

    #[test]
    fn shuffle_round_plays_no_repeated_entry_while_bag_last() {
        let player = FakePlayer::new();
        let mut coord =
            PlaybackCoordinator::with_shuffle(player, FixedShuffle::from_order(vec![2, 0, 1]));
        let s0 = song();
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s0, s1, s2],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::Shuffle);
        // pending_ids after current (s0) = [s1, s2]; len 2 permutation -> [2,0]
        // clamps to identity? Check behavior: advance twice and confirm we
        // consumed two distinct pending entries with no repeat.
        let mut seen = vec![];
        for _ in 0..2 {
            if let Some(id) = coord.advance_to_next() {
                if let Some(song) = coord.current().and_then(|e| e.item.song_id()) {
                    seen.push(song);
                }
                let _ = id;
            }
        }
        // Shuffle must never repeat an entry within a round.
        let mut sorted = seen.clone();
        sorted.sort();
        for w in sorted.windows(2) {
            assert_ne!(w[0], w[1], "shuffle round repeated an entry: {seen:?}");
        }
    }

    #[test]
    fn duplicate_song_in_shuffle_round_stays_distinct() {
        // Same SongId appended twice must never collapse in a shuffle round.
        let player = FakePlayer::new();
        let mut coord =
            PlaybackCoordinator::with_shuffle(player, FixedShuffle::from_order(vec![1, 0, 2]));
        let s = song();
        coord.play_context(&ViewContext {
            songs: vec![s, s, s],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::Shuffle);
        // The queue has 3 distinct shuffle candidates (all same SongId but 3
        // queueEntryIds). A round of 2 advances must consume two different
        // entry ids.
        let mut entry_ids = vec![];
        for _ in 0..2 {
            if let Some(id) = coord.advance_to_next() {
                entry_ids.push(id);
            }
        }
        assert_eq!(entry_ids.len(), 2);
        assert_ne!(
            entry_ids[0], entry_ids[1],
            "duplicate SongId collapsed in shuffle"
        );
    }

    #[test]
    fn repeat_one_repeats_current_on_end() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s = song();
        coord.play_context(&ViewContext {
            songs: vec![s],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::RepeatOne);
        let id = coord.current().unwrap().id;
        let advanced = coord.on_played_to_end();
        assert_eq!(advanced, Some(id), "repeat-one repeats the same entry");
        assert_eq!(coord.current().unwrap().id, id);
    }

    #[test]
    fn explicit_next_leaves_repeat_one() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::RepeatOne);
        // Explicit next must leave repeat-one and go to the next entry.
        coord.advance_to_next();
        assert_eq!(coord.mode(), PlayMode::Sequential);
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
    }

    #[test]
    fn mode_switch_keeps_current_item() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        // Switch through all modes; the current item never changes.
        coord.set_mode(PlayMode::Shuffle);
        coord.set_mode(PlayMode::RepeatOne);
        coord.set_mode(PlayMode::Sequential);
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
    }

    #[test]
    fn previous_restarts_current_after_5_seconds() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        // Simulate being 6 seconds into the current track.
        coord.player().set_position(6.0);
        coord.previous();
        // Current track restarts (still s1), not the previous entry.
        assert_eq!(coord.snapshot().position, Some(0.0));
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
    }

    #[test]
    fn previous_moves_back_within_5_seconds() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        // Start at s1, then advance to s2 (so s1 is in history).
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        coord.advance_to_next();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        // We're on s2, just a moment in (< 5s). Previous goes back to s1.
        coord.player().set_position(2.0);
        coord.previous();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
    }

    // -- Task 8.8: seek / volume / mute via the coordinator -----------------

    #[test]
    fn seek_updates_snapshot_through_coordinator() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        coord.play_context(&ViewContext {
            songs: vec![song()],
            selected_index: 0,
        });
        coord.seek(42.5);
        assert_eq!(coord.snapshot().position, Some(42.5));
    }

    #[test]
    fn set_volume_clamps_and_clears_mute_through_coordinator() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        coord.toggle_mute();
        assert!(coord.snapshot().muted);
        coord.set_volume(0.4);
        assert!((coord.snapshot().volume - 0.4).abs() < f64::EPSILON);
        assert!(!coord.snapshot().muted, "setting volume clears mute");
    }

    #[test]
    fn unmute_restores_last_nonzero_volume_through_coordinator() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        coord.set_volume(0.3);
        coord.toggle_mute(); // mute at 0.3
        assert!(coord.snapshot().muted);
        assert!(
            (coord.snapshot().volume - 0.3).abs() < f64::EPSILON,
            "muting keeps the volume field (mute is only the flag, like mpv)"
        );
        coord.toggle_mute(); // unmute → restores 0.3
        assert!(!coord.snapshot().muted);
        assert!(
            (coord.snapshot().volume - 0.3).abs() < f64::EPSILON,
            "unmute restores the recent non-zero volume"
        );
    }

    #[test]
    fn rejected_property_leaves_snapshot_authoritative() {
        // Command failure rollback (task 8.8): when the backend rejects a
        // write, the snapshot (and thus the UI) must stay consistent — never
        // reflect a position/volume mpv did not reach.
        let player = FakePlayer::new();
        player.fail_next_property(); // simulate backend rejection
        let mut coord = PlaybackCoordinator::new(player);
        coord.play_context(&ViewContext {
            songs: vec![song()],
            selected_index: 0,
        });
        assert_eq!(coord.snapshot().position, Some(0.0));
        coord.seek(9.0);
        assert_eq!(
            coord.snapshot().position,
            Some(0.0),
            "a rejected seek must not advance the snapshot"
        );
    }

    // -- Task 8.7: per-round error set, error advance bypasses repeat-one ----

    #[test]
    fn error_skips_bad_entry_and_plays_next_sequential() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        let s3 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2, s3],
            selected_index: 0,
        });
        // s1 and s3 are fine; s2 is corrupt. After s1 ends, s2 fails and we
        // skip to s3 (never retry s2 this round).
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        coord.advance_to_next(); // → s2
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        let bad = coord.current().unwrap().id;
        let next = coord.on_load_error(); // s2 fails → skip to s3
        assert!(next.is_some());
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s3));
        // The failed entry is recorded (and not revisited this round).
        assert!(coord.failed_round().collect::<Vec<_>>().contains(&bad));
    }

    #[test]
    fn error_bypasses_repeat_one() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::RepeatOne);
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        // The current (repeat-one) entry fails — error advance must NOT loop it;
        // it bypasses repeat-one and goes to the next un-failed entry.
        let next = coord.on_load_error();
        assert!(next.is_some());
        assert_eq!(
            coord.mode(),
            PlayMode::Sequential,
            "error advance leaves repeat-one"
        );
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
    }

    #[test]
    fn error_advance_never_retries_same_entry_in_round() {
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s, s2],
            selected_index: 0,
        });
        // s fails; then s2 (the only other) fails too. With no un-failed entry
        // left, the queue stops rather than re-attempting s (no spin).
        coord.on_load_error(); // s fails → s2
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        let next = coord.on_load_error(); // s2 fails → nothing playable
        assert!(next.is_none());
        assert_eq!(
            coord.snapshot().state,
            echo_core::domain::state::PlaybackState::Stopped
        );
    }

    #[test]
    fn error_advance_all_bad_stops_without_spin() {
        // Every pending entry corrupt: stop after exhausting, never loop.
        let player = FakePlayer::new();
        let mut coord = PlaybackCoordinator::new(player);
        let s1 = song();
        let s2 = song();
        coord.play_context(&ViewContext {
            songs: vec![s1, s2],
            selected_index: 0,
        });
        coord.on_load_error(); // s1 fails → s2
        coord.on_load_error(); // s2 fails → stop
                               // A further error on no current must not resurrect playback.
        coord.on_load_error();
        assert_eq!(
            coord.snapshot().state,
            echo_core::domain::state::PlaybackState::Stopped
        );
        assert_eq!(coord.mode(), PlayMode::Sequential);
    }

    #[test]
    fn error_advance_in_shuffle_skips_failed_entry() {
        // Shuffle only picks un-failed entries: after one fails, the rest of
        // the round visits only the good ones.
        let player = FakePlayer::new();
        let mut coord =
            PlaybackCoordinator::with_shuffle(player, FixedShuffle::from_order(vec![1, 0, 2]));
        let s0 = song();
        let s1 = song();
        let s2 = song();
        let s3 = song();
        coord.play_context(&ViewContext {
            songs: vec![s0, s1, s2, s3],
            selected_index: 0,
        });
        coord.set_mode(PlayMode::Shuffle);
        // pending after s0 = [s1,s2,s3]; FixedShuffle [1,0,2] → plays
        // pending[1]=s2 first, then pending[0]=s1, then pending[2]=s3.
        let first_id = coord.advance_to_next().unwrap();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
        // s2 is corrupt: it fails, gets recorded, and advance skips to the
        // next un-failed entry (s1). It is never revisited this round.
        let failed_id = first_id;
        let next_id = coord.on_load_error().unwrap();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s1));
        assert_ne!(failed_id, next_id, "failed entry is not revisited");
        assert_eq!(coord.failed_round().count(), 1);
        // Moves to s3; the failed s2 stays skipped on the next advance.
        let third = coord.advance_to_next().unwrap();
        assert_eq!(coord.current().unwrap().item.song_id(), Some(s3));
        assert_ne!(third, failed_id);
    }
}
