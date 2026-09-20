//! UI snapshot mapping + forwarder (task 11.2).

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;

use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;

use super::metadata::QueueMetadataResolver;
use super::PlayerPort;
use super::{
    CoordinatorView, QueueEntryMeta, QueueItem, UiLyricLine, UiPlayerSnapshot, UiQueueEntry,
    UiTemporaryLyrics,
};

/// Round a duration in seconds to a whole second for the UI. Fractional
/// seconds are intentional loss (the UI only shows seconds) and negatives are
/// clamped away first, so both clippy casts are deliberate.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn duration_seconds(seconds: f64) -> u64 {
    seconds.max(0.0) as u64
}

/// Build the UI queue rows plus whether the current entry can be imported
/// (a session-only temporary item, task 11.7; a library entry cannot).
/// Extracted from [`map_snapshot`] so that function stays under the
/// file-size lint.
fn build_queue<S: BuildHasher>(
    view: &CoordinatorView,
    metadata: &HashMap<SongId, QueueEntryMeta, S>,
) -> (bool, Vec<UiQueueEntry>) {
    let current_id = view.current.as_ref().map(|e| e.id);
    let current_is_temporary = matches!(
        view.current.as_ref().map(|e| &e.item),
        Some(QueueItem::Temporary(_))
    );
    let queue = view
        .entries
        .iter()
        .map(|e| {
            let is_temporary = matches!(&e.item, QueueItem::Temporary(_));
            let song_meta = e
                .item
                .song_id()
                .and_then(|id| metadata.get(&id))
                .cloned()
                .unwrap_or_default();
            UiQueueEntry {
                entry_id: e.id.to_string(),
                song_id: e.item.song_id().map(|id| id.to_string()),
                title: match &e.item {
                    QueueItem::Library(_) => song_meta.title,
                    QueueItem::Temporary(t) => t
                        .metadata
                        .title
                        .clone()
                        .or_else(|| Some(t.display_name.clone())),
                },
                is_current: Some(e.id) == current_id,
                failed: view.failed_round.contains(&e.id),
                blocked: view.blocked.contains(&e.id),
                can_import: is_temporary,
                artist: match &e.item {
                    QueueItem::Library(_) => song_meta.artist,
                    QueueItem::Temporary(t) => t.metadata.artist.clone(),
                },
                duration_s: match &e.item {
                    QueueItem::Library(_) => song_meta.duration_s,
                    QueueItem::Temporary(t) => {
                        t.metadata.duration.or(t.duration).map(duration_seconds)
                    }
                },
                cover_key: match &e.item {
                    QueueItem::Library(_) => song_meta.cover_key,
                    QueueItem::Temporary(t) => t.metadata.cover_key.clone(),
                },
            }
        })
        .collect();
    (current_is_temporary, queue)
}

/// Map the actor's [`PlayerSnapshot`] (transport state) plus the coordinator's
/// queue view into the UI shape.
///
/// The split matters. The actor knows *how* playback is going (state, position,
/// duration, volume, mute) but nothing about the queue: its commands carry a
/// `SongId`, never a `QueueEntryId`, and it publishes `queue_len: 0` by design.
/// The coordinator owns *what* is playing (the queue, the current entry, the
/// mode). Every queue-derived field of the UI snapshot is therefore read from
/// [`CoordinatorView`] — reading them off the actor's snapshot is what once left
/// the player bar blank (empty 当前播放区 + dead mode button) while a song was
/// audibly playing.
#[must_use]
pub fn map_snapshot<S: BuildHasher>(
    raw: &crate::player::port::PlayerSnapshot,
    view: &CoordinatorView,
    metadata: &HashMap<SongId, QueueEntryMeta, S>,
) -> UiPlayerSnapshot {
    let current = view.current.as_ref();
    let current_id = current.map(|e| e.id);
    let (
        current_song_id,
        current_title,
        current_artist,
        current_album,
        current_cover_key,
        current_lyrics,
    ) = match current.map(|e| &e.item) {
        Some(QueueItem::Library(id)) => {
            // A library song's display title is its resolved metadata — the
            // player bar shows the real title, not a placeholder.
            let title = metadata.get(id).and_then(|meta| meta.title.clone());
            let song_meta = metadata.get(id).cloned().unwrap_or_default();
            (
                Some(id.to_string()),
                title,
                song_meta.artist,
                song_meta.album,
                song_meta.cover_key,
                None,
            )
        }
        Some(QueueItem::Temporary(t)) => (
            None,
            t.metadata
                .title
                .clone()
                .or_else(|| Some(t.display_name.clone())),
            t.metadata.artist.clone(),
            t.metadata.album.clone(),
            t.metadata.cover_key.clone(),
            t.metadata.lyrics.as_ref().map(|lyrics| UiTemporaryLyrics {
                source: lyrics.source.clone(),
                timed: lyrics.timed,
                lines: lyrics
                    .lines
                    .iter()
                    .map(|line| UiLyricLine {
                        seconds: line.seconds,
                        text: line.text.clone(),
                    })
                    .collect(),
                plain_text: lyrics.plain_text.clone(),
                parse_error: lyrics.parse_error.clone(),
            }),
        ),
        None => (None, None, None, None, None, None),
    };
    // A session-only temporary item (no library `song_id`) can be imported into
    // the active library (task 11.7); a library entry cannot.
    let (current_can_import, queue) = build_queue(view, metadata);
    UiPlayerSnapshot {
        state: match raw.state {
            PlaybackState::Stopped => "stopped",
            PlaybackState::Loading => "loading",
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
            PlaybackState::Ended => "ended",
            PlaybackState::Failed => "failed",
        },
        position: raw.position,
        duration: raw.duration,
        volume: raw.volume,
        muted: raw.muted,
        current_queue_entry_id: current_id.map(|q| q.to_string()),
        current_song_id,
        queue_len: view.entries.len(),
        mode: match view.mode {
            super::PlayMode::Sequential => "sequential",
            super::PlayMode::Shuffle => "shuffle",
            super::PlayMode::RepeatOne => "repeatOne",
        },
        current_title,
        current_artist,
        current_album,
        current_cover_key,
        current_lyrics,
        current_can_import,
        queue,
    }
}

/// Bridge a sealed snapshot stream to the UI snapshot path.
///
/// The emitter is moved into a background thread that drains the actor's
/// bounded snapshot subscription and forwards each mapped snapshot. A slow
/// consumer has stale snapshots dropped (the actor's receiver is bounded) so
/// this thread never blocks the actor.
///
/// `queue_provider` returns the coordinator's current view, which supplies
/// every queue-derived field of the UI snapshot (task 11.2) — the actor knows
/// nothing about queue membership. `metadata` resolves the presentation
/// metadata (title/artist/duration/cover) of the queue's library entries,
/// batch-cached so repeated position snapshots never re-query per row (task
/// 2.2).
///
/// # Panics
///
/// If the `echo-snapshot-forwarder` worker thread cannot be spawned. This
/// happens at composition time, before any playback exists, so it is a fatal
/// startup fault rather than a recoverable runtime error.
#[allow(clippy::needless_pass_by_value)] // The port is moved into the forwarder thread.
pub fn spawn_forwarder(
    port: Arc<dyn PlayerPort>,
    queue_provider: Arc<dyn Fn() -> CoordinatorView + Send + Sync>,
    metadata: Arc<QueueMetadataResolver>,
    emit: super::SnapshotEmitter,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-snapshot-forwarder".into())
        .spawn(move || {
            while let Ok(snap) = rx.recv() {
                let view = queue_provider();
                let meta = metadata.resolve(&view.entries);
                emit(map_snapshot(&snap, &view, &meta));
            }
        })
        .expect("spawn snapshot forwarder thread");
}
