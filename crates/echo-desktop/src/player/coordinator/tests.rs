//! `PlaybackCoordinator` cases. Lives beside the module it exercises
//! rather than as an inline block, so the coordinator's own file stays
//! readable as production code.

use super::*;
use crate::player::fake::FakePlayer;
use crate::player::queue::TemporaryItem;
use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;

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
fn next_replays_single_entry_instead_of_reloading_or_pausing() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let s = song();
    coord.play_context(&ViewContext {
        songs: vec![s],
        selected_index: 0,
    });
    coord.seek(24.0);

    let current = coord.queue().current_id();
    assert_eq!(coord.advance_to_next(), current);
    assert_eq!(coord.snapshot().position, Some(0.0));
    assert_eq!(
        coord.snapshot().state,
        echo_core::domain::state::PlaybackState::Playing
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
fn new_view_context_replaces_old_pending_and_priority_lane() {
    // 视图播放是替换而非追加 (spec: 从曲库开始播放 / 视图播放重建队列数量):
    // a fresh context must leave no entry from an older context or its manual
    // "play next" lane behind — no persisted/old-session pending bleeds in.
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let old = song();
    let old2 = song();
    coord.play_context(&ViewContext {
        songs: vec![old, old2],
        selected_index: 0,
    });
    coord.play_next(lib_entry(song())); // a priority-lane entry from the old context
    assert_eq!(coord.queue().priority_ids().len(), 1);

    let s1 = song();
    let s2 = song();
    let s3 = song();
    coord.play_context(&ViewContext {
        songs: vec![s1, s2, s3],
        selected_index: 1,
    });

    assert_eq!(
        coord.current().unwrap().item.song_id(),
        Some(s2),
        "the selected song of the new view is current"
    );
    assert!(
        coord.queue().priority_ids().is_empty(),
        "a new context never inherits the old priority lane"
    );
    let view_ids: Vec<_> = coord
        .queue_view()
        .into_iter()
        .map(|entry| entry.item.song_id().unwrap())
        .collect();
    assert_eq!(
        view_ids,
        vec![s2, s3, s1],
        "the new view's queue starts at the selection and wraps around"
    );
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
    coord.restore_session(queue, PlayMode::Shuffle, 0.42, true, Some(12.5));
    assert_eq!(coord.current().unwrap().item.song_id(), Some(s2));
    assert_eq!(coord.mode(), PlayMode::Shuffle);
    assert_eq!(
        coord.snapshot().state,
        echo_core::domain::state::PlaybackState::Paused
    );
    assert!((coord.snapshot().volume - 0.42).abs() < f64::EPSILON);
    assert!(coord.snapshot().muted);
    assert_eq!(coord.snapshot().position, Some(12.5));
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
    coord.restore_session(build(), PlayMode::Sequential, 0.45, true, None);
    assert!(coord.snapshot().muted, "the first restore mutes");
    coord.restore_session(build(), PlayMode::Sequential, 0.45, true, None);
    assert!(coord.snapshot().muted, "replaying the restore stays muted");
    assert!((coord.snapshot().volume - 0.45).abs() < f64::EPSILON);
}

#[test]
fn prime_context_paused_preserves_settings_without_playing() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let song = song();
    coord.prime_context_paused(
        &ViewContext {
            songs: vec![song],
            selected_index: 0,
        },
        PlayMode::Shuffle,
        0.42,
        true,
    );

    assert_eq!(coord.current().unwrap().item.song_id(), Some(song));
    assert_eq!(coord.queue().len(), 1);
    assert_eq!(coord.mode(), PlayMode::Shuffle);
    assert_eq!(coord.snapshot().state, PlaybackState::Paused);
    assert!((coord.snapshot().volume - 0.42).abs() < f64::EPSILON);
    assert!(coord.snapshot().muted);
    assert_eq!(coord.player().last_loaded_song(), Some(song));
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
        metadata: Default::default(),
        on_active_root: false,
    });
    assert_eq!(coord.current().unwrap().item.song_id(), None);
    assert_eq!(
        coord.current().unwrap().item,
        QueueItem::Temporary(TemporaryItem {
            display_name: "x.mp3".into(),
            path: std::path::PathBuf::from("/tmp/x.mp3"),
            duration: None,
            metadata: Default::default(),
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
fn explicit_next_advances_repeat_one_without_changing_selected_mode() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let s1 = song();
    let s2 = song();
    coord.play_context(&ViewContext {
        songs: vec![s1, s2],
        selected_index: 0,
    });
    coord.set_mode(PlayMode::RepeatOne);
    // Explicit next must use list-loop order while retaining repeat-one.
    coord.advance_to_next();
    assert_eq!(coord.mode(), PlayMode::RepeatOne);
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
fn mode_switch_publishes_snapshot_without_a_current_track() {
    let player = FakePlayer::new();
    let snapshots = player.subscribe_snapshots();
    let mut coord = PlaybackCoordinator::new(player);

    coord.set_mode(PlayMode::Shuffle);

    assert_eq!(coord.mode(), PlayMode::Shuffle);
    assert_eq!(
        snapshots
            .recv()
            .expect("mode change must publish a snapshot")
            .mode,
        PlayMode::Shuffle
    );
}

#[test]
fn selecting_a_queue_entry_makes_it_current_and_loads_it() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let first = song();
    let selected = song();
    coord.play_context(&ViewContext {
        songs: vec![first, selected],
        selected_index: 0,
    });
    let selected_entry = coord.queue_view()[1].id;

    assert_eq!(coord.play_queue_entry(selected_entry), Some(selected_entry));
    assert_eq!(coord.current().map(|entry| entry.id), Some(selected_entry));
    assert_eq!(coord.player().last_loaded_song(), Some(selected));
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

#[test]
fn previous_replays_single_entry_instead_of_pausing() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let s = song();
    coord.play_context(&ViewContext {
        songs: vec![s],
        selected_index: 0,
    });
    coord.seek(24.0);

    coord.previous();
    assert_eq!(coord.snapshot().position, Some(0.0));
    assert_eq!(
        coord.snapshot().state,
        echo_core::domain::state::PlaybackState::Playing
    );
}

#[test]
fn recovered_blocked_entry_becomes_eligible_after_retry() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let s1 = song();
    let s2 = song();
    coord.play_context(&ViewContext {
        songs: vec![s1, s2],
        selected_index: 0,
    });
    let blocked = coord.queue().pending_ids()[0];
    coord.queue_mut().set_blocked(blocked, true);
    assert_eq!(
        coord.advance_to_next(),
        Some(coord.queue().current_id().unwrap()),
        "blocked item is skipped"
    );
    coord.retry_blocked(|song| song == s2);
    assert!(!coord.queue().is_blocked(blocked));
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
    assert!(coord.failed_round().any(|x| x == bad));
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
        PlayMode::RepeatOne,
        "error skip must not mutate the selected playback mode"
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
