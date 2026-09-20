use super::*;

use std::collections::HashSet;

#[test]
fn map_snapshot_maps_states_and_modes() {
    let raw = PlayerSnapshot {
        state: PlaybackState::Playing,
        position: Some(12.5),
        duration: Some(240.0),
        volume: 0.7,
        muted: false,
        queue_len: 3,
        mode: PlayMode::Shuffle,
    };
    let view = CoordinatorView {
        entries: vec![],
        failed_round: HashSet::default(),
        blocked: HashSet::default(),
        current: None,
        mode: PlayMode::Shuffle,
    };
    let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
    assert_eq!(ui.state, "playing");
    assert_eq!(ui.position, Some(12.5));
    assert_eq!(ui.mode, "shuffle");
    assert_eq!(ui.current_song_id, None);
    assert_eq!(ui.current_title, None);
    assert!(ui.queue.is_empty());
}

#[test]
fn map_snapshot_derives_current_song_id_from_library_entry() {
    let song = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Paused,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        entries: vec![entry.clone()],
        failed_round: HashSet::default(),
        blocked: HashSet::default(),
        current: Some(entry.clone()),
        mode: PlayMode::Sequential,
    };
    let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
    assert_eq!(ui.current_title, None);
    assert_eq!(ui.queue.len(), 1);
    assert_eq!(ui.queue[0].entry_id, entry.id.to_string());
    assert_eq!(ui.queue[0].song_id, Some(song.to_string()));
    assert!(ui.queue[0].is_current);
    assert!(!ui.queue[0].failed);
}

#[test]
fn ui_queue_identity_comes_from_the_coordinator_not_the_transport_snapshot() {
    // Regression for the blank player bar: the real actor never sets a
    // queue-entry id (its `LoadLibrarySong` carries only a `SongId`), so a UI
    // snapshot that read it from `PlayerSnapshot` produced
    // `currentQueueEntryId: null` — the bar rendered its empty 当前播放区 and
    // disabled transport while the song was audibly playing. Even with a bare
    // transport snapshot (no queue fields at all) and a stale
    // `queue_len`/`mode`, the UI must report the coordinator's truth.
    let song = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Playing,
        position: Some(1.0),
        queue_len: 0,               // what the actor publishes
        mode: PlayMode::Sequential, // what the actor publishes
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        entries: vec![entry.clone()],
        failed_round: HashSet::default(),
        blocked: HashSet::default(),
        current: Some(entry.clone()),
        mode: PlayMode::RepeatOne,
    };
    let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
    assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert_eq!(ui.queue_len, 1, "queue length is the coordinator's queue");
    assert_eq!(ui.mode, "repeatOne", "mode is the coordinator's mode");
}

#[test]
fn map_snapshot_surfaces_temporary_title_without_song_id() {
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Temporary(crate::player::queue::TemporaryItem {
            display_name: "outside.m4a".into(),
            path: "/tmp/outside.m4a".into(),
            duration: None,
            metadata: Default::default(),
            on_active_root: false,
        }),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Failed,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        entries: vec![entry.clone()],
        failed_round: HashSet::default(),
        blocked: HashSet::default(),
        current: Some(entry),
        mode: PlayMode::Sequential,
    };
    let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
    assert_eq!(ui.current_song_id, None);
    assert_eq!(ui.current_title, Some("outside.m4a".into()));
    assert_eq!(ui.state, "failed");
    assert_eq!(ui.queue[0].title.as_deref(), Some("outside.m4a"));
    assert_eq!(ui.queue[0].song_id, None);
    // A temporary item can be imported into the active library (task 11.7).
    assert!(ui.current_can_import);
    assert!(ui.queue[0].can_import);
}

#[test]
fn map_snapshot_flags_failed_entries_and_distinguishes_current() {
    let s1 = SongId::new();
    let current = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(s1),
    };
    let s2 = SongId::new();
    let failed = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(s2),
    };
    let entries = vec![current.clone(), failed.clone()];
    let failed_round = std::collections::HashSet::from([failed.id]);
    let raw = PlayerSnapshot {
        state: PlaybackState::Playing,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        entries,
        failed_round,
        blocked: HashSet::default(),
        current: Some(current),
        mode: PlayMode::Sequential,
    };
    let ui = map_snapshot(&raw, &view, &std::collections::HashMap::new());
    assert!(ui.queue[0].is_current);
    assert!(!ui.queue[0].failed);
    assert!(!ui.queue[1].is_current);
    assert!(ui.queue[1].failed);
    assert_eq!(ui.queue[1].song_id, Some(s2.to_string()));
    // Library entries are never importable-as-temporary (task 11.7).
    assert!(!ui.current_can_import);
    assert!(!ui.queue[0].can_import);
    assert!(!ui.queue[1].can_import);
}

#[test]
fn map_snapshot_surfaces_blocked_queue_entries() {
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(SongId::new()),
    };
    let view = CoordinatorView {
        entries: vec![entry.clone()],
        failed_round: HashSet::default(),
        blocked: std::collections::HashSet::from([entry.id]),
        current: Some(entry),
        mode: PlayMode::Sequential,
    };
    assert!(
        map_snapshot(
            &PlayerSnapshot::default(),
            &view,
            &std::collections::HashMap::new()
        )
        .queue[0]
            .blocked
    );
}
