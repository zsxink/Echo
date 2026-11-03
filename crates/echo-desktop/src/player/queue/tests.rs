//! `Queue` cases (ordering, lanes, history, removal).

use super::*;

use crate::player::port::PlayMode;

fn lib(s: SongId) -> QueueEntry {
    QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(s),
    }
}

fn song() -> SongId {
    SongId::new()
}

/// Assert that `mutate` advances the queue revision.
///
/// The revision is the transport's only signal that the projection changed,
/// so *every* projection-changing mutator must move it forward. A redundant
/// bump is harmless (one extra projection delivery); a missing one freezes
/// the panel on a stale queue.
fn assert_advances(queue: &mut Queue, label: &str, mutate: impl FnOnce(&mut Queue)) {
    let before = queue.revision();
    mutate(queue);
    let after = queue.revision();
    assert!(
        after > before,
        "mutator `{label}` must advance the queue revision ({before:?} -> {after:?})"
    );
}

/// A queue with three library entries, the first one current. Returns the
/// queue together with the entry ids and song ids in append order.
fn seeded_queue() -> (Queue, Vec<QueueEntryId>, Vec<SongId>) {
    let mut q = Queue::new();
    let mut ids = Vec::new();
    let mut songs = Vec::new();
    for _ in 0..3 {
        let s = song();
        songs.push(s);
        ids.push(q.push(lib(s)));
    }
    q.set_current(ids[0]);
    (q, ids, songs)
}

#[test]
fn queue_revision_advances_on_every_projection_changing_mutation() {
    // spec 「队列修订号的单调性」: every mutator that can change the projection
    // advances the revision. The revision is the transport's only signal for
    // "the queue moved" — a missing bump silently freezes the panel.
    let (mut q, ids, songs) = seeded_queue();
    assert_eq!(Queue::new().revision(), QueueRevision::INITIAL);

    assert_advances(&mut q, "set_priority", |q| {
        q.set_priority(vec![ids[1]]);
    });
    assert_advances(&mut q, "set_shuffle", |q| {
        q.set_shuffle(true, vec![ids[1], ids[2]]);
    });
    assert_advances(&mut q, "refresh_shuffle_bag", |q| {
        q.refresh_shuffle_bag(vec![ids[2]]);
    });
    assert_advances(&mut q, "restore_history", |q| {
        q.restore_history(vec![HistoryRecord {
            entry_id: ids[0],
            played_at_ms: now_ms(),
        }]);
    });
    assert_advances(&mut q, "set_blocked", |q| {
        q.set_blocked(ids[2], true);
    });
    assert_advances(&mut q, "push", |q| {
        q.push(lib(song()));
    });
    assert_advances(&mut q, "insert_next", |q| {
        q.insert_next(lib(song()));
    });
    assert_advances(&mut q, "promote_to_priority", |q| {
        // The same song as an existing entry: the mutator promotes that entry
        // instead of appending a duplicate.
        q.promote_to_priority(lib(songs[1]));
    });
    assert_advances(&mut q, "advance_priority", |q| {
        q.advance_priority();
    });
    assert_advances(&mut q, "set_current", |q| {
        q.set_current(ids[1]);
    });
    assert_advances(&mut q, "advance_next", |q| {
        q.advance_next();
    });
    assert_advances(&mut q, "advance_in_mode", |q| {
        q.advance_in_mode(PlayMode::Sequential);
    });
    assert_advances(&mut q, "previous", |q| {
        q.previous();
    });
    assert_advances(&mut q, "previous_in_loop", |q| {
        q.previous_in_loop();
    });
    assert_advances(&mut q, "remove", |q| {
        q.remove(ids[2]);
    });
    assert_advances(&mut q, "remove_song", |q| {
        q.remove_song(songs[2]);
    });
    assert_advances(&mut q, "remove_direct", |q| {
        q.remove_direct(ids[0]);
    });
    assert_advances(&mut q, "clear_pending", |q| {
        q.clear_pending();
    });
}

#[test]
fn queue_revision_is_unchanged_by_reads() {
    // Position ticks poll the queue constantly; reading it must never look
    // like a change, or the transport would re-ship the projection at 10 Hz.
    let (q, ids, _) = seeded_queue();
    let before = q.revision();
    let _ = q.view_entries();
    let _ = q.current();
    let _ = q.pending();
    let _ = q.get(ids[0]);
    assert_eq!(q.revision(), before);
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
fn queue_projection_order_and_content_unchanged() {
    // spec 「投影顺序与内容不变」: the O(n) rewrite must not move a single
    // entry. This pins the whole projection contract in one place: current
    // first, the manual lane ahead of the ordinary entries, the shuffle round
    // order for the rest, blocked entries dropped, duplicates kept.
    let repeated = song();
    let mut q = Queue::new();
    let current = q.push(lib(repeated));
    let ordinary = q.push(lib(song()));
    let blocked = q.push(lib(song()));
    let duplicate = q.push(lib(repeated));
    q.set_current(current);
    q.set_blocked(blocked, true);

    // Sequential mode: current first, then the pending entries in list order.
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, ordinary, duplicate]);

    // The manual lane precedes the ordinary entries.
    let lane = q.insert_next(lib(song()));
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, lane, ordinary, duplicate]);

    // Shuffle mode: the round order wins for the ordinary entries, the lane
    // still comes first, and the blocked entry stays out.
    q.set_shuffle(true, vec![duplicate, ordinary]);
    let ids: Vec<_> = q.view_entries().into_iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![current, lane, duplicate, ordinary]);

    // The projection carries the entries themselves, not just their ids.
    let view = q.view_entries();
    assert_eq!(view[0].id, current);
    assert_eq!(view[0].item.song_id(), Some(repeated));
    assert_eq!(view[2].item.song_id(), Some(repeated));
    assert!(
        view.iter().all(|entry| entry.id != blocked),
        "a blocked entry must never reach the projection"
    );
}

#[test]
fn queue_projection_stays_within_budget_for_ten_thousand_entries() {
    // spec 「大队列的投影构造保持在预算内」: the projection must be built in
    // time proportional to the entry count. The budget is generous enough to
    // survive a slow two-core CI runner — an O(n²) implementation misses it by
    // two orders of magnitude.
    const ENTRIES: usize = 10_000;
    const BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

    let mut q = Queue::new();
    for _ in 0..ENTRIES {
        q.push(lib(song()));
    }
    let first = q.entries()[0].id;
    q.set_current(first);
    // A shuffled round and a manual lane exercise every lookup path of the
    // projection, not just the plain list order.
    let pending = q.pending_ids();
    q.set_shuffle(true, pending);
    q.insert_next(lib(song()));

    let started = std::time::Instant::now();
    let view = q.view_entries();
    let elapsed = started.elapsed();

    assert_eq!(
        view.len(),
        ENTRIES + 1,
        "every entry must reach the projection (the manual lane adds one more)"
    );
    assert!(
        elapsed < BUDGET,
        "building the projection of {ENTRIES} entries took {elapsed:?}, over the {BUDGET:?} budget"
    );
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
