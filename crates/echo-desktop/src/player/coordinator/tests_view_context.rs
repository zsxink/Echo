//! View-context playback cases: mode persistence across a new context, context
//! replacement semantics, and the typed conflict an empty view reports. Lives
//! beside the module it exercises; `tests.rs` keeps the remaining coordinator
//! cases.

use super::*;
use crate::player::fake::FakePlayer;

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
fn play_context_keeps_the_user_mode() {
    // 设计: 记忆播放模式 — a new context never resets the mode back to
    // sequential (the old hardcode did exactly that on every play click).
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    coord.set_mode(PlayMode::RepeatOne);
    coord
        .play_context(&ViewContext {
            songs: vec![song(), song()],
            selected_index: 0,
        })
        .unwrap();
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
    coord
        .play_context(&ViewContext {
            songs: vec![old, old2],
            selected_index: 0,
        })
        .unwrap();
    coord.play_next(lib_entry(song())); // a priority-lane entry from the old context
    assert_eq!(coord.queue().priority_ids().len(), 1);

    let s1 = song();
    let s2 = song();
    let s3 = song();
    coord
        .play_context(&ViewContext {
            songs: vec![s1, s2, s3],
            selected_index: 1,
        })
        .unwrap();

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
fn playing_an_empty_view_context_reports_a_conflict() {
    // A view can legitimately be empty (a filtered library with no matches);
    // asking to play it must come back as a typed conflict, not an index
    // panic, and must leave the coordinator untouched.
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let error = coord
        .play_context(&ViewContext {
            songs: Vec::new(),
            selected_index: 0,
        })
        .unwrap_err();
    assert!(
        matches!(error, echo_core::error::Error::Conflict { .. }),
        "expected a conflict, got {error:?}"
    );
    assert_eq!(error.code(), "conflict");
    assert!(coord.current().is_none());
    assert!(coord.queue().view_entries().is_empty());
    assert_eq!(coord.player().last_loaded_song(), None);
}

#[test]
fn priming_an_empty_view_context_reports_a_conflict() {
    let player = FakePlayer::new();
    let mut coord = PlaybackCoordinator::new(player);
    let error = coord
        .play_context_paused(&ViewContext {
            songs: Vec::new(),
            selected_index: 0,
        })
        .unwrap_err();
    assert!(
        matches!(error, echo_core::error::Error::Conflict { .. }),
        "expected a conflict, got {error:?}"
    );
    assert_eq!(error.code(), "conflict");
    assert!(coord.current().is_none());
    assert_eq!(coord.player().last_loaded_song(), None);
}
