//! UI snapshot mapping + forwarder (task 11.2).
//!
//! Three streams leave this module, each with its own change criterion:
//!
//! - `player://snapshot` — the 10 Hz **transport** state. It carries no queue
//!   entry and no effects snapshot, so its payload size does not depend on the
//!   library size.
//! - `player://queue` — the queue projection, published only when the queue
//!   revision changed.
//! - `player://effects` — the effects snapshot, published only when its
//!   sequence advanced (which, after the sequence became a *content change*
//!   counter, means the content actually changed).

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;

use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;

use super::metadata::QueueMetadataResolver;
use super::PlayerPort;
use super::{
    CoordinatorView, PlayerEmitters, PlayerFrameProvider, QueueEntryMeta, QueueItem,
    QueueProjection, UiLyricLine, UiPlayerSnapshot, UiQueueEntry, UiQueueProjection,
    UiTemporaryLyrics,
};
use crate::player::coordinator::PlaybackCoordinator;
use crate::player::queue::QueueRevision;

/// Round a duration in seconds to a whole second for the UI. Fractional
/// seconds are intentional loss (the UI only shows seconds) and negatives are
/// clamped away first, so both clippy casts are deliberate.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn duration_seconds(seconds: f64) -> u64 {
    seconds.max(0.0) as u64
}

/// Map a queue projection into the UI rows the queue panel renders.
///
/// The metadata is passed in rather than resolved here so the caller can
/// decide *when* the (repository-backed) resolution happens: the projection is
/// built only when the queue revision changed, so the resolution must not run
/// on every position frame.
#[must_use]
pub fn map_queue_projection<S: BuildHasher>(
    projection: &QueueProjection,
    metadata: &HashMap<SongId, QueueEntryMeta, S>,
) -> UiQueueProjection {
    let current_id = projection.current;
    let entries = projection
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
                failed: projection.failed_round.contains(&e.id),
                blocked: projection.blocked.contains(&e.id),
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
    UiQueueProjection {
        revision: projection.revision.value(),
        entries,
    }
}

/// Map a queue projection into the UI shape, resolving the presentation
/// metadata of its library entries.
///
/// The whole-queue resolution is the expensive part of the projection, which is
/// why it only runs when the queue revision changed.
#[must_use]
pub fn ui_queue_projection(
    projection: &QueueProjection,
    metadata: &QueueMetadataResolver,
) -> UiQueueProjection {
    let resolved = metadata.resolve(&projection.entries);
    map_queue_projection(projection, &resolved)
}

/// Build the current queue projection from the coordinator, under one lock.
///
/// The revision and the entries are read together, so a consumer never sees a
/// revision that does not belong to the entries it carries.
///
/// # Panics
///
/// Only if the coordinator mutex is poisoned — a thread panicked while holding
/// it, which indicates a bug in the playback coordinator.
#[must_use]
pub fn queue_projection<P: PlayerPort>(
    coordinator: &std::sync::Mutex<PlaybackCoordinator<P>>,
) -> QueueProjection {
    let coord = coordinator.lock().expect("player coordinator lock");
    QueueProjection {
        revision: coord.queue_revision(),
        entries: coord.queue_view(),
        failed_round: coord.failed_round().collect(),
        blocked: coord.blocked_entries().into_iter().collect(),
        current: coord.current().map(|entry| entry.id),
    }
}

/// Map the actor's [`PlayerSnapshot`] (transport state) plus the coordinator's
/// current entry into the UI shape.
///
/// The split matters. The actor knows *how* playback is going (state, position,
/// duration, volume, mute) but nothing about the queue: its commands carry a
/// `SongId`, never a `QueueEntryId`, and it publishes `queue_len: 0` by design.
/// The coordinator owns *what* is playing (the queue, the current entry, the
/// mode). Every queue-derived field of the UI snapshot is therefore read from
/// [`CoordinatorView`] — reading them off the actor's snapshot is what once left
/// the player bar blank (empty 当前播放区 + dead mode button) while a song was
/// audibly playing.
///
/// `current_meta` is the resolved presentation metadata of the current entry —
/// one row, not the whole queue. The queue itself is published on its own
/// event.
#[must_use]
pub fn map_snapshot(
    raw: &crate::player::port::PlayerSnapshot,
    view: &CoordinatorView,
    current_meta: Option<&QueueEntryMeta>,
    queue_len: usize,
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
            let song_meta = current_meta.cloned().unwrap_or_default();
            (
                Some(id.to_string()),
                song_meta.title,
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
    let current_can_import = matches!(current.map(|e| &e.item), Some(QueueItem::Temporary(_)));
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
        queue_len,
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
        queue_revision: view.revision.value(),
    }
}

/// Whether an effects snapshot must be published again.
///
/// The effects snapshot carries a monotonic sequence that advances when the
/// *content* changed, so an unchanged snapshot is identified by its sequence.
/// A snapshot without a sequence carries no change evidence — it is published
/// and the consumer applies its own ordering rules.
const fn effects_changed(previous: Option<u64>, next: Option<u64>) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => next > previous,
        _ => true,
    }
}

/// What the forwarder already delivered, so it can tell what changed.
#[derive(Default)]
struct Delivered {
    /// The queue revision the consumer last received.
    queue_revision: Option<QueueRevision>,
    /// The number of entries in the last delivered projection.
    queue_len: usize,
    /// The effects sequence the consumer last received.
    effects_sequence: Option<u64>,
}

/// Bridge a sealed snapshot stream to the UI snapshot path.
///
/// The emitter is moved into a background thread that drains the actor's
/// bounded snapshot subscription and forwards each mapped snapshot. A slow
/// consumer has stale snapshots dropped (the actor's receiver is bounded) so
/// this thread never blocks the actor.
///
/// `frames` supplies the coordinator's state for one frame. It receives the
/// revision the consumer last received, so the queue projection is built only
/// when the queue actually changed — and it is built under the same lock as
/// the revision, so the two never disagree. `metadata` resolves the
/// presentation metadata (title/artist/duration/cover) of the queue's library
/// entries, batch-cached so repeated position snapshots never re-query per row
/// (task 2.2).
///
/// # Panics
///
/// If the `echo-snapshot-forwarder` worker thread cannot be spawned. This
/// happens at composition time, before any playback exists, so it is a fatal
/// startup fault rather than a recoverable runtime error.
#[allow(clippy::needless_pass_by_value)] // The port is moved into the forwarder thread.
pub fn spawn_forwarder(
    port: Arc<dyn PlayerPort>,
    frames: PlayerFrameProvider,
    metadata: Arc<QueueMetadataResolver>,
    effects: super::EffectsSnapshotProvider,
    emit: PlayerEmitters,
) {
    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-snapshot-forwarder".into())
        .spawn(move || {
            let mut delivered = Delivered::default();
            while let Ok(snap) = rx.recv() {
                let frame = frames(delivered.queue_revision);
                // The projection goes first: the queue length it carries is
                // then already known when the position frame is mapped.
                if let Some(projection) = frame.projection {
                    let ui = ui_queue_projection(&projection, &metadata);
                    (emit.queue)(ui);
                    delivered.queue_revision = Some(projection.revision);
                    delivered.queue_len = projection.entries.len();
                }
                let current_meta = frame
                    .view
                    .current
                    .as_ref()
                    .and_then(|entry| entry.item.song_id())
                    .map(|id| metadata.resolve_one(id));
                (emit.snapshot)(map_snapshot(
                    &snap,
                    &frame.view,
                    current_meta.as_ref(),
                    delivered.queue_len,
                ));
                let effects_snapshot = (effects)();
                if effects_changed(
                    delivered.effects_sequence,
                    effects_snapshot.snapshot_sequence,
                ) {
                    delivered.effects_sequence = effects_snapshot.snapshot_sequence;
                    (emit.effects)(effects_snapshot);
                }
            }
        })
        .expect("spawn snapshot forwarder thread");
}
