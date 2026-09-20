//! `Queue` cases (ordering, lanes, history, removal).

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
    let song_id = song();
    let mut queue = Queue::new();
    let first = queue.push(lib(song_id));
    let second = queue.push(lib(song_id));
    let third = queue.push(lib(song_id));
    // Three distinct QueueEntryIds for the same SongId.
    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_ne!(first, third);
    assert_eq!(queue.distinct_song_ids(), vec![song_id]);
    assert_eq!(queue.len(), 3);
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

/// 列表循环下的"上一首"（DP-R11-S01）。
///
/// The requirement is explicit that list-loop must walk the **playback context
/// position** (first item wrapping to the last) and must never substitute the
/// random/cross-mode history. `previous_returns_history_entry` above covers the
/// history path, and `coordinator::tests::previous_replays_single_entry_...`
/// covers the single-entry dispatch — between them they leave the actual walk
/// and its wrap unproven, which is why this case exists.
#[test]
fn previous_in_loop_walks_the_context_backwards_and_wraps_to_the_tail() {
    let mut q = Queue::new();
    let a = q.push(lib(song()));
    let b = q.push(lib(song()));
    let c = q.push(lib(song()));
    q.set_current(b);
    let queued_next = q.insert_next(lib(song()));

    // 切到上下文的上一项，且不消费手动待播项。
    assert_eq!(q.previous_in_loop(), Some(a));
    assert_eq!(q.current_id(), Some(a));
    assert_eq!(q.priority_ids(), &[queued_next]);
    // 按新的当前位置重新投影待播项：当前位置置顶，顺序保持。
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![a, queued_next, b, c]);

    // 首项回绕至末项。
    q.set_current(a);
    assert_eq!(q.previous_in_loop(), Some(c));
    assert_eq!(q.current_id(), Some(c));
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![c, queued_next, a, b]);
}

#[test]
fn view_projection_keeps_current_first_and_includes_loop_wrap() {
    let mut q = Queue::new();
    let first = q.push(lib(song()));
    let current = q.push(lib(song()));
    let pending = q.push(lib(song()));
    q.set_current(first);
    q.set_current(current);

    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, pending, first]);
}

#[test]
fn play_next_lane_is_fifo_and_projection_precedes_normal_entries() {
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    let normal = q.push(lib(song()));
    q.set_current(current);
    let a = q.insert_next(lib(song()));
    let b = q.insert_next(lib(song()));
    let c = q.insert_next(lib(song()));

    assert_eq!(q.priority_ids(), &[a, b, c]);
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, a, b, c, normal]);
    assert_eq!(q.advance_priority(), Some(a));
    assert_eq!(q.advance_priority(), Some(b));
    assert_eq!(q.advance_priority(), Some(c));
    assert_eq!(q.advance_priority(), None);
}

#[test]
fn blocked_or_removed_priority_entries_never_reach_projection_or_consumer() {
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    q.set_current(current);
    let blocked = q.insert_next(lib(song()));
    let removed = q.insert_next(lib(song()));
    q.set_blocked(blocked, true);
    assert!(q.remove(removed));

    assert!(q.priority_ids().is_empty());
    assert_eq!(
        q.view_entries()
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![current]
    );
    assert_eq!(q.advance_priority(), None);
}

#[test]
fn clear_pending_keeps_only_current_even_after_loop_wrap() {
    let mut q = Queue::new();
    let first = q.push(lib(song()));
    let current = q.push(lib(song()));
    let last = q.push(lib(song()));
    q.set_current(current);
    q.insert_next(lib(song()));
    q.clear_pending();

    assert_eq!(q.entries().len(), 1);
    assert_eq!(q.current_id(), Some(current));
    assert!(!q.contains(first));
    assert!(!q.contains(last));
    assert!(q.priority_ids().is_empty());
}

#[test]
fn view_projection_uses_shuffle_order_without_collapsing_duplicate_songs() {
    let repeated = song();
    let mut q = Queue::new();
    let current = q.push(lib(repeated));
    let first_pending = q.push(lib(repeated));
    let second_pending = q.push(lib(song()));
    q.set_current(current);
    q.set_shuffle(true, vec![second_pending, first_pending]);

    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, second_pending, first_pending]);
    assert_ne!(
        current, first_pending,
        "duplicate songs retain queue-entry identity"
    );
}

#[test]
fn view_projection_reflects_insert_next_and_clear_pending() {
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    let tail = q.push(lib(song()));
    q.set_current(current);
    let next = q.insert_next(lib(song()));
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, next, tail]);

    q.clear_pending();
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current]);
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
    let duplicate = song(); // same song, two entries
    let other = song();
    let third_song = song();
    let mut queue = Queue::new();
    let first_entry = queue.push(lib(duplicate));
    let second_entry = queue.push(lib(other));
    let third_entry = queue.push(lib(third_song));
    queue.set_current(first_entry);
    // A round of three distinct entry ids.
    queue.set_shuffle(true, vec![second_entry, third_entry, first_entry]);
    let mut seen = vec![];
    while let Some(id) = queue.advance_in_mode(PlayMode::Shuffle) {
        seen.push(id);
    }
    // Duplicate SongId entries (first and second differ by id even if same
    // song) never fold together — a round visits each entry id once.
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

fn temp(path: &str) -> QueueItem {
    QueueItem::Temporary(Box::new(TemporaryItem {
        display_name: "tmp".into(),
        path: std::path::PathBuf::from(path),
        duration: None,
        metadata: TemporaryMetadata::default(),
        on_active_root: false,
    }))
}

#[test]
fn priority_lane_physical_contiguity() {
    // The lane is a contiguous slice right after the current entry. Inserting
    // A, B, C must keep every lane member adjacent to current and ahead of the
    // ordinary pending entries — re-insert must never fall between old members.
    let s1 = song();
    let normal1 = song();
    let normal2 = song();
    let mut q = Queue::new();
    let current = q.push(lib(s1));
    let n1 = q.push(lib(normal1));
    let n2 = q.push(lib(normal2));
    q.set_current(current);

    let a = q.insert_next(lib(song()));
    let b = q.insert_next(lib(song()));
    let c = q.insert_next(lib(song()));

    // Physical order and lane are identical: current, lane, then ordinary.
    assert_eq!(q.priority_ids(), &[a, b, c]);
    assert_eq!(
        q.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![current, a, b, c, n1, n2]
    );

    // Clearing pending keeps only the current entry.
    q.clear_pending();
    assert_eq!(q.entries().len(), 1);
    assert_eq!(q.current_id(), Some(current));
    assert!(q.priority_ids().is_empty());
}

#[test]
fn insert_next_after_moved_current_keeps_lane_contiguous() {
    // Session restore / play_queue_entry can move current to an entry that sits
    // ahead of stored lane members (physically earlier).  The next insert must
    // still land after *all* lane members, never between them.
    let mut q = Queue::new();
    let head = q.push(lib(song()));
    let cur = q.push(lib(song()));
    q.set_current(cur);
    let x = q.insert_next(lib(song()));
    let y = q.insert_next(lib(song()));
    // Physically: head, cur, x, y.
    assert_eq!(
        q.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![head, cur, x, y]
    );

    // Restore/skip makes `head` the current again — current now sits *before*
    // the lane instead of right before it.
    q.set_current(head);
    let z = q.insert_next(lib(song()));
    assert_eq!(q.priority_ids(), &[x, y, z]);
    // The lane still follows current (head) as a contiguous slice: head, x, y,
    // z, cur — cur trails as an ordinary/restored member, never interleaving.
    assert_eq!(
        q.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![head, x, y, z, cur]
    );
    assert_eq!(q.current_id(), Some(head));
}

#[test]
fn remove_keeps_priority_lane_consistent() {
    let s1 = song();
    let normal = song();
    let mut q = Queue::new();
    q.push(lib(s1)); // will be current
    let n1 = q.push(lib(normal));
    let cur_id = q.entries()[0].id;
    q.set_current(cur_id);
    let a = q.insert_next(lib(song()));
    let b = q.insert_next(lib(song()));

    // Remove a lane member: lane keeps order and stays contiguous.
    assert!(q.remove(a));
    assert_eq!(q.priority_ids(), &[b]);
    // Insert another lane member must go after b.
    let c = q.insert_next(lib(song()));
    assert_eq!(q.priority_ids(), &[b, c]);
    assert_eq!(
        q.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![cur_id, b, c, n1]
    );

    // Remove an ordinary pending entry: current/lane unaffected.
    assert!(q.remove(n1));
    assert_eq!(q.priority_ids(), &[b, c]);
    let view: Vec<_> = q.view_entries().into_iter().map(|e| e.id).collect();
    assert_eq!(view, vec![cur_id, b, c]);
}

#[test]
fn promote_to_priority_moves_existing_entry_to_lane_front() {
    let target = song();
    let s1 = song();
    let mut q = Queue::new();
    let current = q.push(lib(s1));
    let normal = q.push(lib(target)); // target is an ordinary pending entry
    let tail = q.push(lib(song()));
    q.set_current(current);
    assert_eq!(q.pending_ids(), vec![normal, tail]);

    // "Play next" the target: no duplicate is created — the existing entry is
    // promoted to the lane front.
    let promoted = q.promote_to_priority(lib(target));
    assert_eq!(promoted, normal);
    assert_eq!(q.priority_ids(), &[normal]);
    // It now fronts the lane, immediately after current; tail stays pending.
    let view: Vec<_> = q.view_entries().into_iter().map(|e| e.id).collect();
    assert_eq!(view, vec![current, normal, tail]);
    // No new entry: queue length unchanged.
    assert_eq!(q.entries().len(), 3);
}

#[test]
fn promote_to_priority_is_idempotent_for_same_song() {
    let s = song();
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    q.set_current(current);
    let promoted = [
        q.promote_to_priority(lib(s)),
        q.promote_to_priority(lib(s)),
        q.promote_to_priority(lib(s)),
    ];
    // Repeated "next" of one song collapses to a single lane copy.
    assert_eq!(promoted[0], promoted[1]);
    assert_eq!(promoted[1], promoted[2]);
    assert_eq!(q.priority_ids(), &[promoted[0]]);
    assert_eq!(q.entries().len(), 2); // current + the one promoted entry
}

#[test]
fn promote_to_priority_appends_when_not_queued() {
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    q.set_current(current);
    let a = q.promote_to_priority(lib(song()));
    let b = q.promote_to_priority(lib(song()));
    assert_eq!(q.priority_ids(), &[a, b]);
    assert_eq!(q.entries().len(), 3);
}

#[test]
fn promote_matches_temporary_by_path() {
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    q.set_current(current);
    let _ordinary = q.push(QueueEntry {
        id: QueueEntryId::new(),
        item: temp("/tmp/a.mp3"),
    });
    // Same path already queued → promote, no duplicate.
    let promoted = q.promote_to_priority(QueueEntry {
        id: QueueEntryId::new(),
        item: temp("/tmp/a.mp3"),
    });
    assert_eq!(q.priority_ids(), vec![promoted]);
    assert_eq!(q.entries().len(), 2); // current + the promoted temp

    // A different temp path is a distinct identity → appended.
    let other = q.promote_to_priority(QueueEntry {
        id: QueueEntryId::new(),
        item: temp("/tmp/b.mp3"),
    });
    assert_eq!(q.priority_ids(), vec![promoted, other]);
    assert_eq!(q.entries().len(), 3);
}

#[test]
fn promote_does_not_fold_ordinary_duplicates_or_current() {
    let s = song();
    let mut q = Queue::new();
    q.push(lib(s)); // ordinary duplicate #1 (also the one we promote)
    let current = q.push(lib(song()));
    q.set_current(current);
    // Two "加入队列" duplicates for s in the ordinary area.
    let d1 = q.push(lib(s));
    let d2 = q.push(lib(s));

    // Promote one copy to the lane; the other stays an ordinary member.
    let promoted = q.promote_to_priority(lib(s));
    assert_eq!(q.priority_ids(), vec![promoted]);
    assert!(q.is_pending(d1)); // still pending
    assert!(q.is_pending(d2)); // still pending
    assert_ne!(promoted, d1);
    assert_ne!(promoted, d2);
    assert_eq!(q.entries().len(), 4);

    // Promoting the *current* song is a no-op that adds nothing.
    let current_song_id = q.current().unwrap().item.song_id().unwrap();
    let n = q.promote_to_priority(lib(current_song_id));
    assert_eq!(q.entries().len(), 4);
    assert_eq!(n, current);
}

#[test]
fn restore_with_duplicate_lane_ids_for_one_song_collapses_on_promote() {
    // spec 「优先区重复副本清理」: a restored session can carry two lane IDs
    // that both refer to the same song.  The next "play next" involving that
    // song must collapse the lane to a single copy at the front.
    let s = song();
    let mut q = Queue::new();
    let current = q.push(lib(song()));
    q.set_current(current);
    // Two ordinary entries for s (a restored session kept both).
    let e1 = q.push(lib(s));
    let e2 = q.push(lib(s));
    // The stale session put BOTH of them in the priority lane.
    q.set_priority(vec![e1, e2]);

    assert_eq!(q.priority_ids(), &[e1, e2]);
    let promoted = q.promote_to_priority(lib(s));
    // Lane now has exactly one copy, fronted by the promoted entry.
    assert_eq!(q.priority_ids(), vec![promoted]);
    // One of the two entries remains an ordinary pending member.
    let pending: Vec<_> = q.pending_ids();
    assert_eq!(pending.len(), 2); // promoted + the other duplicate
    assert_ne!(promoted, e2); // one of the original lane copies is ordinary now
}
