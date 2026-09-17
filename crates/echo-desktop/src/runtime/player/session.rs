//! Session persistence wiring: saver thread + cold-start restore (task 8.9).

use std::sync::{Arc, Mutex};

use echo_core::domain::state::PlaybackState;

use super::PlaybackCoordinator;
use super::PlayerPort;
use super::SessionPersistence;
use super::ViewContext;
use crate::player::session::{rebuild_queue, snapshot_queue};

/// Spawn the session-saver thread (task 8.9 落盘接线): persist the durable
/// playback session — queue, current, history, shuffle bag, mode, volume,
/// mute, last position and the playing source (哪个歌单) — to the atomic
/// desktop-state store.
///
/// Write policy (throttled, never hot):
/// - every **state transition** saves once (a pause/stop/track change is the
///   moment that matters for a crash);
/// - a **volume / mute** change saves once its burst settles (see
///   `AUDIO_SETTLE`), *without* waiting for `min_interval`;
/// - while `Playing`, at most one save per `min_interval` (position progress
///   does not justify an fsync per 10 Hz snapshot).
///
/// The audio case is why the loop wakes on a timeout instead of blocking on the
/// mailbox: 暂停后调音量 (or 静音) leaves *no* follow-up snapshot, so a policy of
/// "save on the next snapshot, throttled" never wrote it — the stored session
/// kept the pre-adjustment volume / `muted:true` and replayed that stale value
/// on the next start.
///
/// A save failure is swallowed (logged only) — persistence must never take
/// playback down with it.
///
/// # Panics
///
/// If the `echo-session-saver` worker thread cannot be spawned (composition
/// time, before any playback exists), and inside that thread if `min_interval`
/// exceeds `Instant::now()` since the clock's epoch — the `checked_sub` below
/// underflows. Both are startup/precondition faults, not runtime conditions.
#[allow(clippy::needless_pass_by_value)] // The port is moved into the saver thread.
pub fn spawn_session_saver(
    port: Arc<dyn PlayerPort>,
    coordinator: Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    persistence: Arc<dyn SessionPersistence>,
    source: Arc<std::sync::Mutex<Option<String>>>,
    min_interval: std::time::Duration,
) {
    /// How long a volume/mute burst must stay quiet before it is written.
    /// Dragging the slider publishes ~10 snapshots per second; waiting out that
    /// gap collapses the whole drag into one save, while the value the user let
    /// go on is still the one that lands.
    const AUDIO_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
    /// The save loop's wake-up period — short enough that a settled audio
    /// change is flushed promptly after the last snapshot.
    const SAVER_TICK: std::time::Duration = std::time::Duration::from_millis(100);

    let rx = port.subscribe_snapshots();
    std::thread::Builder::new()
        .name("echo-session-saver".into())
        .spawn(move || {
            let mut last_state = PlaybackState::Stopped;
            let mut last_audio: Option<(f64, bool)> = None;
            let mut last_save = std::time::Instant::now().checked_sub(min_interval).unwrap();
            // An audio change observed but not yet written, waiting out its
            // settle window. It carries the snapshot to write, because by the
            // time the window closes no further snapshot has arrived.
            let mut pending_audio: Option<(
                crate::player::port::PlayerSnapshot,
                std::time::Instant,
            )> = None;

            let save = |snap: &crate::player::port::PlayerSnapshot| {
                let session = {
                    let Ok(coord) = coordinator.lock() else {
                        return;
                    };
                    snapshot_queue(
                        coord.queue(),
                        coord.mode(),
                        snap.volume,
                        snap.muted,
                        snap.position,
                        source.lock().ok().and_then(|s| s.clone()).as_deref(),
                    )
                };
                let _ = persistence.save(Some(&session));
            };

            loop {
                match rx.recv_timeout(SAVER_TICK) {
                    Ok(raw) => {
                        let state_changed = raw.state != last_state;
                        let audio_changed = last_audio.is_some_and(|(volume, muted)| {
                            (raw.volume - volume).abs() > super::VOLUME_EPSILON
                                || raw.muted != muted
                        });
                        last_state = raw.state;
                        last_audio = Some((raw.volume, raw.muted));

                        if audio_changed {
                            pending_audio = Some((raw.clone(), std::time::Instant::now()));
                        }
                        if state_changed {
                            // A transition (暂停 / 停止 / 换曲) is the moment a
                            // crash would lose the most: write at once. It also
                            // supersedes any audio change still settling.
                            save(&raw);
                            last_save = std::time::Instant::now();
                            pending_audio = None;
                            continue;
                        }
                        if raw.state == PlaybackState::Playing
                            && last_save.elapsed() >= min_interval
                        {
                            save(&raw);
                            last_save = std::time::Instant::now();
                            pending_audio = None;
                            continue;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // The player is gone (app quitting): flush a settled
                        // audio change so the last thing the user did survives.
                        if let Some((snap, _)) = &pending_audio {
                            save(snap);
                        }
                        break;
                    }
                }

                if let Some((snap, changed_at)) = &pending_audio {
                    if changed_at.elapsed() >= AUDIO_SETTLE {
                        save(snap);
                        last_save = std::time::Instant::now();
                        pending_audio = None;
                    }
                }
            }
        })
        .expect("spawn session saver thread");
}

/// The restore-or-prime cold-start decision (task 8.9 + 默认态设计):
///
/// 1. A persisted session with entries is restored: the queue is rebuilt,
///    mode/volume/mute re-applied, and the current entry loaded **paused**.
/// 2. With nothing to restore but the library has songs, the *default* kicks
///    in: the first song of 全部歌曲 (as given by `default_view_songs`) is
///    primed into the 播放控制栏 paused, in 列表循环.
/// 3. An empty library yields an empty queue — the bar stays empty.
///
/// `default_view_songs` is called at most once, only in case 2.
pub fn restore_or_prime_playback(
    coordinator: &Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>,
    persistence: &dyn SessionPersistence,
    restore_verdicts: impl FnOnce(
        &crate::player::session::PlaybackSession,
    ) -> Vec<crate::player::session::RestoreVerdict>,
    default_view_songs: impl FnOnce() -> Vec<echo_core::domain::ids::SongId>,
) -> &'static str {
    let restored = match persistence.load() {
        Ok(Some(session)) if !session.entries.is_empty() => Some(session),
        _ => None,
    };
    if let Some(session) = restored {
        let (queue, _summary) = rebuild_queue(&session, &restore_verdicts(&session));
        let volume = session.volume;
        let muted = session.muted;
        let mode = session.mode;
        let position = session.position;
        if let Ok(mut coord) = coordinator.lock() {
            coord.restore_session(queue, mode, volume, muted, position);
        }
        return "restored";
    }
    // Nothing persisted: 默认全部歌曲第一条入栏 (paused, never a sound).
    let songs = default_view_songs();
    if songs.is_empty() {
        return "empty";
    }
    let ctx = ViewContext {
        songs,
        selected_index: 0,
    };
    if let Ok(mut coord) = coordinator.lock() {
        coord.play_context_paused(&ctx);
    }
    "primed"
}
