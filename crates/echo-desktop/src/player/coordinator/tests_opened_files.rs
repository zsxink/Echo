//! Batch file-open / window-drop queue cases (#117). Lives beside the module
//! it exercises; `tests.rs` holds the pre-existing coordinator cases.

use super::*;
use crate::player::fake::FakePlayer;
use crate::player::queue::TemporaryMetadata;
use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;

fn song() -> SongId {
    SongId::new()
}

// --- Window file drop / multi-file system open (#117) ------------------------

fn temporary(name: &str) -> OpenedPlayItem {
    OpenedPlayItem::Temporary(Box::new(TemporaryPlay {
        display_name: name.to_owned(),
        path: std::path::PathBuf::from("/tmp").join(name),
        duration: None,
        metadata: TemporaryMetadata::default(),
        on_active_root: false,
    }))
}

fn temporary_name(entry: &QueueEntry) -> Option<&str> {
    match &entry.item {
        QueueItem::Temporary(item) => Some(item.display_name.as_str()),
        QueueItem::Library(_) => None,
    }
}

#[test]
fn opened_batch_creates_one_entry_per_path_in_order() {
    let mut coord = PlaybackCoordinator::new(FakePlayer::new());
    coord.play_opened_files(vec![
        temporary("01.flac"),
        temporary("02.mp3"),
        temporary("03.wav"),
    ]);
    let entries = coord.queue().entries();
    assert_eq!(entries.len(), 3, "N paths must produce N queue entries");
    let names: Vec<_> = entries
        .iter()
        .map(|entry| temporary_name(entry).expect("temporary entry"))
        .collect();
    assert_eq!(names, vec!["01.flac", "02.mp3", "03.wav"]);
    assert_eq!(coord.current().map(|entry| entry.id), Some(entries[0].id));
    // A single load sessions is bound to the first entry. Calling
    // `play_temporary` once per path would bind it to the last (and leave one
    // entry) — this is the regression #117 fixes.
    assert_eq!(
        coord.active_load_session().map(|(id, _)| id),
        Some(entries[0].id)
    );
    assert_eq!(coord.snapshot().state, PlaybackState::Playing);
}

#[test]
fn opened_batch_mixes_library_and_temporary_in_input_order() {
    let first = song();
    let last = song();
    let mut coord = PlaybackCoordinator::new(FakePlayer::new());
    coord.play_opened_files(vec![
        OpenedPlayItem::Library(first),
        temporary("outside.flac"),
        OpenedPlayItem::Library(last),
    ]);
    let entries = coord.queue().entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].item.song_id(), Some(first));
    assert_eq!(entries[1].item.song_id(), None);
    assert_eq!(entries[2].item.song_id(), Some(last));
    assert_eq!(
        coord.current().and_then(|entry| entry.item.song_id()),
        Some(first)
    );
    assert_eq!(coord.player().last_loaded_song(), Some(first));
}

#[test]
fn opened_batch_replaces_the_current_queue_and_priority_lane() {
    let mut coord = PlaybackCoordinator::new(FakePlayer::new());
    coord.play_context(&ViewContext {
        songs: vec![song(), song(), song()],
        selected_index: 1,
    });
    coord.on_load_error();
    assert_eq!(coord.failed_round().count(), 1);
    let revision = coord.queue_revision();

    coord.play_opened_files(vec![temporary("a.flac"), temporary("b.flac")]);
    let entries = coord.queue().entries();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.item.song_id().is_none()));
    assert!(coord.queue().priority_ids().is_empty());
    assert_eq!(coord.failed_round().count(), 0);
    assert!(coord.queue_revision() > revision);
}

#[test]
fn single_temporary_open_is_still_a_one_item_batch() {
    let mut coord = PlaybackCoordinator::new(FakePlayer::new());
    coord.play_temporary(TemporaryPlay {
        display_name: "x.mp3".to_owned(),
        path: std::path::PathBuf::from("/tmp/x.mp3"),
        duration: None,
        metadata: TemporaryMetadata::default(),
        on_active_root: false,
    });
    assert_eq!(coord.queue().entries().len(), 1);
    assert_eq!(coord.current().and_then(temporary_name), Some("x.mp3"));
}

#[test]
fn empty_opened_batch_leaves_the_queue_untouched() {
    let mut coord = PlaybackCoordinator::new(FakePlayer::new());
    coord.play_opened_files(vec![temporary("a.flac")]);
    let before = coord.queue().entries().to_vec();
    let revision = coord.queue_revision();
    coord.play_opened_files(Vec::new());
    assert_eq!(coord.queue().entries(), before.as_slice());
    assert_eq!(coord.queue_revision(), revision);
}
