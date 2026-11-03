use super::*;

use std::collections::HashSet;

/// A transport view with no current entry.
fn empty_view(mode: PlayMode) -> CoordinatorView {
    CoordinatorView {
        current: None,
        mode,
        revision: QueueRevision::INITIAL,
    }
}

/// A queue projection over `entries`, with `current` as the current entry.
fn projection(entries: Vec<QueueEntry>, current: Option<QueueEntryId>) -> QueueProjection {
    QueueProjection {
        revision: QueueRevision::INITIAL.next(),
        entries,
        failed_round: HashSet::default(),
        blocked: HashSet::default(),
        current,
    }
}

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
    let ui = map_snapshot(&raw, &empty_view(PlayMode::Shuffle), None, 0);
    assert_eq!(ui.state, "playing");
    assert_eq!(ui.position, Some(12.5));
    assert_eq!(ui.mode, "shuffle");
    assert_eq!(ui.current_song_id, None);
    assert_eq!(ui.current_title, None);
    assert_eq!(ui.queue_len, 0);
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
        current: Some(entry.clone()),
        mode: PlayMode::Sequential,
        revision: QueueRevision::INITIAL,
    };
    let ui = map_snapshot(&raw, &view, None, 1);
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
    assert_eq!(ui.current_title, None);
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
        current: Some(entry.clone()),
        mode: PlayMode::RepeatOne,
        revision: QueueRevision::INITIAL.next(),
    };
    let ui = map_snapshot(&raw, &view, None, 1);
    assert_eq!(ui.current_queue_entry_id, Some(entry.id.to_string()));
    assert_eq!(ui.current_song_id, Some(song.to_string()));
    assert_eq!(ui.queue_len, 1, "queue length is the coordinator's queue");
    assert_eq!(ui.mode, "repeatOne", "mode is the coordinator's mode");
}

#[test]
fn map_snapshot_surfaces_temporary_title_without_song_id() {
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Temporary(Box::new(crate::player::queue::TemporaryItem {
            display_name: "outside.m4a".into(),
            path: "/tmp/outside.m4a".into(),
            duration: None,
            metadata: crate::player::queue::TemporaryMetadata::default(),
            on_active_root: false,
        })),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Failed,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        current: Some(entry),
        mode: PlayMode::Sequential,
        revision: QueueRevision::INITIAL,
    };
    let ui = map_snapshot(&raw, &view, None, 1);
    assert_eq!(ui.current_song_id, None);
    assert_eq!(ui.current_title, Some("outside.m4a".into()));
    assert_eq!(ui.state, "failed");
    // A temporary item can be imported into the active library (task 11.7).
    assert!(ui.current_can_import);
}

#[test]
fn snapshot_payload_carries_no_queue_entries_or_effects() {
    // The 10 Hz payload must not carry the queue projection or the effects
    // snapshot: that is what kept it at ~148 KB for a 480-song library
    // (issue #112). The queue and the effects travel on their own events.
    let song = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song),
    };
    let raw = PlayerSnapshot {
        state: PlaybackState::Playing,
        ..PlayerSnapshot::default()
    };
    let view = CoordinatorView {
        current: Some(entry),
        mode: PlayMode::Sequential,
        revision: QueueRevision::INITIAL.next(),
    };
    let value = serde_json::to_value(map_snapshot(&raw, &view, None, 480)).expect("serialize");
    let object = value.as_object().expect("snapshot object");
    assert!(
        !object.contains_key("queue"),
        "the transport snapshot must not carry the queue projection"
    );
    assert!(
        !object.contains_key("effects"),
        "the transport snapshot must not carry the effects snapshot"
    );
    assert!(
        object.contains_key("queueRevision"),
        "the transport snapshot carries the queue revision"
    );
}

#[test]
fn snapshot_payload_size_does_not_depend_on_the_queue_length() {
    // The size of the position stream must be a property of the transport,
    // not of the library: a 480-song queue must not make the payload bigger.
    let song = SongId::new();
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(song),
    };
    let raw = PlayerSnapshot::default();
    let view = CoordinatorView {
        current: Some(entry),
        mode: PlayMode::Sequential,
        revision: QueueRevision::INITIAL,
    };
    let small = serde_json::to_string(&map_snapshot(&raw, &view, None, 1)).expect("serialize");
    let large = serde_json::to_string(&map_snapshot(&raw, &view, None, 480)).expect("serialize");
    assert!(
        large.len() - small.len() <= 8,
        "the transport payload must not grow with the queue length ({} vs {} bytes)",
        small.len(),
        large.len()
    );
    assert!(
        small.len() < 1024,
        "the transport payload stays in the KB range ({} bytes)",
        small.len()
    );
}

#[test]
fn map_queue_projection_flags_failed_entries_and_distinguishes_current() {
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
    let mut projection = projection(vec![current.clone(), failed.clone()], Some(current.id));
    projection.failed_round = std::collections::HashSet::from([failed.id]);
    let ui = map_queue_projection(&projection, &std::collections::HashMap::new());
    assert!(ui.entries[0].is_current);
    assert!(!ui.entries[0].failed);
    assert!(!ui.entries[1].is_current);
    assert!(ui.entries[1].failed);
    assert_eq!(ui.entries[1].song_id, Some(s2.to_string()));
    // Library entries are never importable-as-temporary (task 11.7).
    assert!(!ui.entries[0].can_import);
    assert!(!ui.entries[1].can_import);
}

#[test]
fn map_queue_projection_surfaces_blocked_queue_entries() {
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(SongId::new()),
    };
    let mut projection = projection(vec![entry.clone()], Some(entry.id));
    projection.blocked = std::collections::HashSet::from([entry.id]);
    let ui = map_queue_projection(&projection, &std::collections::HashMap::new());
    assert!(ui.entries[0].blocked);
}

#[test]
fn map_queue_projection_carries_the_revision_it_was_built_at() {
    let entry = QueueEntry {
        id: QueueEntryId::new(),
        item: QueueItem::Library(SongId::new()),
    };
    let projection = projection(vec![entry], None);
    let ui = map_queue_projection(&projection, &std::collections::HashMap::new());
    assert_eq!(ui.revision, projection.revision.value());
}
