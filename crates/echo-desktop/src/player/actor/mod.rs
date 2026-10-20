//! Dedicated OS-thread actor that owns the player backend (task 8.2).
//!
//! The libmpv handle is created on a dedicated thread and never moves off it.
//! Commands arrive over a **bounded** channel ([`mpsc::sync_channel`]); the
//! actor loop processes them and pumps the backend for events, publishing a
//! normalized [`PlayerSnapshot`] after each meaningful change.
//!
//! The actor depends only on a small [`Backend`] trait, so the loop machinery
//! — **bounded command channel, generation discard, ordered destruction** — is
//! testable with a [`TestBackend`] that needs no libmpv. The real
//! [`MpvBackend`] wraps the isolated `unsafe` FFI module ([`super::ffi`]).
//!
//! **Generation discard:** every load increments `generation`. A load marked
//! `Loading` supersedes any prior track's events; a late `FileLoaded` from an
//! old generation cannot overwrite the new track's state because the actor
//! only ever publishes the *current* generation's snapshot (see `run`).
//!
//! **Ordered destruction:** `Shutdown` stops the loop and calls
//! [`Backend::terminate`] on the actor thread before the thread joins. Because
//! the handle never leaves the actor thread, destruction is single-owner and
//! ordered — no concurrent free, exactly one `terminate_destroy`.
//!
//! ## Module map (split from the former single-file actor.rs)
//!
//! - [`backend`] — `Backend` trait, `BackendEvent`, `BackendProperty`,
//!   `OBSERVED_PROPERTIES`.
//! - [`harden_options`] — the audio-only mpv option set + `extra_options`.
//! - [`mpv_backend`] — the real libmpv-backed implementation.
//! - [`test_backend`] — the scripted backend used by `tests`.
//! - [`tests`] — the unit tests.

#![allow(unsafe_code)] // Calls into the documented, isolated libmpv FFI adapter.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;
use echo_core::error::Error;

use super::ffi;
#[path = "../actor_effects.rs"]
mod effects;
#[path = "../native_effects.rs"]
mod native_effects;
use super::port::{
    PlayMode, PlayerCommand, PlayerError, PlayerPort, PlayerSnapshot, VOLUME_EPSILON,
};

mod backend;
mod harden_options;
mod mpv_backend;
#[cfg(test)]
mod test_backend;

use backend::Backend;
#[cfg(test)]
use backend::OBSERVED_PROPERTIES;
pub use backend::{BackendEvent, BackendProperty};
#[cfg(test)]
use harden_options::{HARDENED_OPTIONAL, HARDENED_REQUIRED};
use mpv_backend::MpvBackend;
#[cfg(test)]
use test_backend::TestBackend;

/// Capacity of the bounded command channel.
pub const COMMAND_CAPACITY: usize = 64;

/// Resolves a library [`SongId`] to an absolute file path. The resolver is
/// built over the repository + root registry in the composition root and
/// injected into the actor so that `LoadLibrarySong` can resolve and load
/// files on the actor thread (never leaking absolute paths to DTOs).
///
/// Returns `Err` when the song or its root is unknown / unbound.
pub type SongResolver = Arc<dyn Fn(SongId) -> Result<std::path::PathBuf, Error> + Send + Sync>;

// ---------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------

/// Handle to the background actor, held by the coordinator. Sends commands on
/// the bounded channel; reads snapshots; awaits ordered teardown.
pub struct PlayerActor {
    tx: mpsc::SyncSender<PlayerCommand>,
    snapshot: Arc<RwLock<PlayerSnapshot>>,
    /// Live snapshot subscribers, each with a bounded mailbox (capacity 1). The
    /// actor loop fans every published snapshot out to them; the composition
    /// root registers the UI forwarder here. Without a *live* sender the
    /// `player://snapshot` stream is silently empty and every snapshot-driven
    /// surface (player bar, queue, progress) freezes at its initial value.
    subscribers: Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>>,
    stopped: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    effects: effects::Shared,
}

impl PlayerActor {
    /// Spawn the actor bound to a real libmpv backend loaded from `libmpv_path`.
    ///
    /// `resolver` maps a library [`SongId`] to an absolute file path on the
    /// actor thread; it is `None` in test paths that only exercise
    /// `LoadTemporary` / command routing. The production composition root
    /// always supplies a resolver so that `LoadLibrarySong` can resolve and
    /// load files without leaking absolute paths to DTOs.
    ///
    /// # Errors
    ///
    /// [`FfiSpawnError::Load`] if libmpv cannot be loaded or resolved (missing
    /// file, unresolvable `@rpath` dependency, ABI-drifted symbol) — surfaced
    /// synchronously so a broken load fails startup instead of degrading to a
    /// silent `Stopped` that reports play transitions with no audio;
    /// [`FfiSpawnError::Spawn`] if the dedicated thread cannot be created.
    ///
    /// The preflight dlopens the library and resolves every pinned symbol on the
    /// calling thread, then drops it (owned by `MpvSys`, released on drop). The
    /// actor re-opens it on its own thread so the handle stays thread-confined.
    /// This is cheap (one dlopen) and covers the load failures that previously
    /// only surfaced as a logged `drain_without_backend`.
    pub fn spawn_mpv(
        libmpv_path: &Path,
        snapshot: Arc<RwLock<PlayerSnapshot>>,
        resolver: Option<SongResolver>,
    ) -> Result<Self, FfiSpawnError> {
        // SAFETY: `MpvSys::load` only dlopens and resolves symbols; the loaded
        // library is owned by the returned `MpvSys` (`_lib`), which is dropped
        // at the end of this scope. No handle is created here.
        unsafe { ffi::MpvSys::load(libmpv_path) }.map_err(FfiSpawnError::Load)?;
        let path = libmpv_path.to_owned();
        Self::spawn_with(
            move || {
                // SAFETY: the actor thread owns the MpvBackend from here on.
                unsafe { MpvBackend::open(&path) }
            },
            snapshot,
            resolver,
        )
    }

    /// Spawn with a caller-supplied backend factory. Generic over the backend
    /// so tests inject a [`TestBackend`]; the production path uses `MpvBackend`.
    fn spawn_with<F, B>(
        backend_factory: F,
        snapshot: Arc<RwLock<PlayerSnapshot>>,
        resolver: Option<SongResolver>,
    ) -> Result<Self, FfiSpawnError>
    where
        F: FnOnce() -> Result<B, ffi::HandleError> + Send + 'static,
        B: Backend + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<PlayerCommand>(COMMAND_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_join = stopped.clone();
        let snapshot_join = snapshot.clone();
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        let subscribers_join = subscribers.clone();
        let effects = effects::Shared::default();
        let effects_join = effects.clone();

        let join = std::thread::Builder::new()
            .name("echo-mpv-actor".into())
            .spawn(move || {
                let backend = match backend_factory() {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(error = %e, "mpv actor: backend unavailable, disabled");
                        effects_join.close();
                        drain_without_backend(rx, &snapshot_join, &stopped_join);
                        return;
                    }
                };
                let mut loop_state =
                    ActorLoop::new(backend, snapshot_join.clone(), subscribers_join);
                loop_state.effects = effects::Engine::new(effects_join.clone());
                loop_state.resolver = resolver;
                loop_state.run(rx);
                // Loop exits only on Shutdown / backend shutdown; ordered
                // teardown happens here, on the actor thread, before joining.
                effects_join.close();
                loop_state.backend.terminate();
                stopped_join.store(true, Ordering::Release);
            })
            .map_err(FfiSpawnError::Spawn)?;

        Ok(Self {
            tx,
            snapshot,
            subscribers,
            stopped,
            join: Some(join),
            effects,
        })
    }

    /// Ordered shutdown: stop accepting commands, terminate the backend on the
    /// actor thread, join the thread. Blocks until teardown completes.
    /// Takes `&mut self` (not `self`) so a caller can observe
    /// [`Self::is_stopped`] after teardown; idempotent.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.stopped.store(true, Ordering::Release);
    }

    /// Whether the actor has fully shut down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl Drop for PlayerActor {
    /// Ordered teardown if the caller did not run [`Self::shutdown`] explicitly
    /// (e.g. the actor is moved around by the runtime). Prevents a leaked
    /// actor thread at process exit.
    fn drop(&mut self) {
        if self.join.is_some() && !self.is_stopped() {
            let _ = self.tx.send(PlayerCommand::Shutdown);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            self.stopped.store(true, Ordering::Release);
        }
    }
}

impl PlayerPort for PlayerActor {
    fn effects_submit(
        &self,
        revision: u64,
        enabled: bool,
        payload: crate::effects::Payload,
    ) -> Result<crate::effects::EffectsRuntime, PlayerError> {
        self.effects.submit(revision, enabled, payload)
    }
    fn effects_runtime(&self) -> crate::effects::EffectsRuntime {
        self.effects.runtime()
    }
    fn effects_bypass(&self, revision: u64) -> Result<crate::effects::EffectsRuntime, PlayerError> {
        self.effects.bypass(revision)
    }

    fn send(&self, cmd: PlayerCommand) -> Result<(), PlayerError> {
        self.tx.try_send(cmd).map_err(|_| {
            // A full or closed channel means the actor cannot accept work now.
            PlayerError::ActorClosed
        })
    }

    fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot
            .read()
            .expect("actor snapshot poisoned")
            .clone()
    }

    fn subscribe_snapshots(&self) -> mpsc::Receiver<PlayerSnapshot> {
        // A real, bounded mailbox: the actor loop fans every published snapshot
        // into it. The previous "never-delivering placeholder" here made the
        // whole `player://snapshot` stream a no-op in production — the player
        // played fine while every snapshot-driven surface stayed blank.
        let (tx, rx) = mpsc::sync_channel(1);
        self.subscribers
            .lock()
            .expect("player subscribers poisoned")
            .push(tx);
        rx
    }
}

/// Errors spawning the actor.
#[derive(Debug, thiserror::Error)]
pub enum FfiSpawnError {
    /// libmpv could not be loaded/resolved (missing file, unresolvable `@rpath`
    /// dependency, or an ABI-drifted symbol). Surfaced synchronously at spawn so
    /// a broken load fails startup instead of degrading to a silent `Stopped`.
    #[error("bundled libmpv could not be loaded: {0}")]
    Load(#[source] ffi::MpvLoadError),
    #[error("failed to spawn the libmpv actor thread: {0}")]
    Spawn(std::io::Error),
}

/// The actor loop: owns the backend, processes commands, pumps events.
#[allow(clippy::struct_excessive_bools)] // These are orthogonal actor snapshots/intent flags, not combinable modes.
struct ActorLoop<B: Backend> {
    backend: B,
    generation: u64,
    state: PlaybackState,
    position: Option<f64>,
    duration: Option<f64>,
    snapshot: Arc<RwLock<PlayerSnapshot>>,
    /// The subscribers every published snapshot is fanned out to. Shared with
    /// the [`PlayerActor`] handle so `subscribe_snapshots` can register more.
    subscribers: Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>>,
    volume: f64,
    muted: bool,
    /// The last non-zero volume, so unmuting restores it (task 8.8: mute 记住
    /// 最近非零值). Persists the user's loudness choice across a mute/unmute
    /// while the volume field itself drops toward 0 only when muted.
    last_nonzero_volume: f64,
    mode: PlayMode,
    /// App foreground flag driving the snapshot throttle (10 Hz fg / 1 Hz bg).
    foreground: bool,
    /// When the last *property* snapshot was published, for throttling.
    last_property_publish: std::time::Instant,
    /// Resolves a library `SongId` → absolute file path on the actor thread.
    /// `None` in tests that only exercise `LoadTemporary` / command routing;
    /// the production composition root always supplies a resolver.
    resolver: Option<SongResolver>,
    /// The pause state the most recent load / transport command **wants** mpv
    /// to be in.
    ///
    /// This exists because mpv's runtime `pause` is **sticky**: the manual is
    /// explicit that "if any option is changed at runtime (via input commands),
    /// they are not reset when a new file is played" (Per-File Options). So the
    /// `pause=yes` a 冷启动 prime writes keeps silencing *every later*
    /// `loadfile` until something writes `pause=no` — the actor's own `state`
    /// flag moving to `Playing` was never enough, because it is not the audio.
    ///
    /// Re-asserted on every [`BackendEvent::FileLoaded`] (the first moment the
    /// flag provably belongs to the new file) and used to decide which state to
    /// publish, so 点歌真的出声 and 暂停态的加载不会被画成正在播放。
    intended_paused: bool,
    /// A seek received during asynchronous file loading. mpv resets its
    /// position as `loadfile` completes, so this is applied on `FileLoaded`.
    pending_seek: Option<f64>,
    effects: effects::Engine,
    effects_held: bool,
    /// Independent of the snapshot: a rejected hold cannot count as silence.
    effects_pause_confirmed: bool,
    load_completed: bool,
}

impl<B: Backend> ActorLoop<B> {
    fn new(
        backend: B,
        snapshot: Arc<RwLock<PlayerSnapshot>>,
        subscribers: Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>>,
    ) -> Self {
        Self {
            backend,
            generation: 0,
            state: PlaybackState::Stopped,
            position: None,
            duration: None,
            snapshot,
            subscribers,
            volume: 1.0,
            muted: false,
            last_nonzero_volume: 1.0,
            mode: PlayMode::Sequential,
            foreground: true,
            // Backdate so the first property event always publishes (avoids a
            // dropped first `time-pos` right after startup / a load).
            last_property_publish: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
            resolver: None,
            // A fresh actor has nothing loaded, so "not paused" is the only
            // intent that can be true once a file actually loads.
            intended_paused: false,
            pending_seek: None,
            effects: effects::Engine::new(effects::Shared::default()),
            effects_held: false,
            effects_pause_confirmed: false,
            load_completed: false,
        }
    }

    /// The throttle interval for continuous (property-driven) snapshot
    /// refreshes: 10 Hz foreground, 1 Hz background (task 8.4).
    const fn property_interval(&self) -> std::time::Duration {
        if self.foreground {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_secs(1)
        }
    }

    /// Persist a *discrete* snapshot immediately (track change, seek, pause,
    /// mute, volume, ended). `gen` is the generation this snapshot was observed
    /// under, used by the coordinator to tag/reject stale IPC events (8.4).
    fn publish(&self, gen: u64) {
        let snap = self.build_snapshot();
        *self.snapshot.write().expect("actor snapshot poisoned") = snap.clone();
        self.fanout(&snap);
        let _ = gen;
        tracing::debug!(state = ?self.state, "mpv actor snapshot");
    }

    /// Persist a *property-driven* snapshot only if the throttle interval has
    /// elapsed since the last one. This keeps continuous `time-pos`
    /// refresh bounded (no event flood over a 10-minute track) while discrete
    /// transitions still publish immediately via [`Self::publish`].
    fn publish_throttled(&mut self, gen: u64) {
        let interval = self.property_interval();
        let now = std::time::Instant::now();
        if now.duration_since(self.last_property_publish) >= interval {
            self.last_property_publish = now;
            let snap = self.build_snapshot();
            *self.snapshot.write().expect("actor snapshot poisoned") = snap.clone();
            self.fanout(&snap);
            tracing::debug!(state = ?self.state, "mpv actor property snapshot (throttled)");
        }
        let _ = gen;
    }

    /// Hand a freshly published snapshot to every live subscriber.
    ///
    /// Each subscriber owns a **bounded** mailbox (capacity 1): a mailbox that
    /// is still full means the consumer has not drained the previous snapshot
    /// yet. A snapshot carries the whole player state, so skipping the stale one
    /// is coalescing rather than data loss — and, crucially, the actor never
    /// blocks on a slow consumer. Subscribers whose receiver is gone are pruned
    /// here, so a dropped forwarder cannot leak or stall the actor.
    fn fanout(&self, snap: &PlayerSnapshot) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("player subscribers poisoned");
        subscribers.retain(|tx| match tx.try_send(snap.clone()) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        });
    }

    const fn build_snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            state: self.state,
            position: self.position,
            duration: self.duration,
            volume: self.volume,
            muted: self.muted,
            queue_len: 0, // queue membership is coordinator-owned (8.5)
            mode: self.mode,
        }
    }

    /// The transport state mpv's `pause` flag implies for a snapshot the flag
    /// can actually express: `Playing` ⇄ `Paused`, or `None` when the flag
    /// changes nothing.
    ///
    /// `Playing`/`Paused` are the *only* states the flag describes. A late
    /// `pause=yes` echo must never rewrite an `Ended`/`Stopped`/`Failed`
    /// snapshot into `Paused`: that would resurrect a finished track in the UI
    /// (progress bar back at 0, a queue that already ran out showing as loaded).
    fn transport_from_pause(current: PlaybackState, paused: bool) -> Option<PlaybackState> {
        if !matches!(current, PlaybackState::Playing | PlaybackState::Paused) {
            return None;
        }
        let next = if paused {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };
        (next != current).then_some(next)
    }

    #[allow(clippy::needless_pass_by_value)] // The receiver is intentionally owned by the actor thread.
    #[allow(clippy::too_many_lines)] // The actor loop keeps command, effects, and event ordering visible in one place.
    fn run(&mut self, rx: mpsc::Receiver<PlayerCommand>) {
        loop {
            // Drain bounded commands first.
            while let Ok(cmd) = rx.try_recv() {
                if !self.handle_command(cmd) {
                    return; // Shutdown
                }
            }
            // A rejected pause never becomes a confirmed hold merely because
            // FileLoaded or a UI snapshot calls this track paused. Retry before
            // pumping a queued load or releasing the first-sample barrier.
            if self.effects_held && !self.effects_pause_confirmed {
                self.effects_pause_confirmed =
                    self.backend.write_property(BackendProperty::Pause(true));
                if self.effects_pause_confirmed
                    && (self.state == PlaybackState::Playing
                        || (self.state == PlaybackState::Loading && self.load_completed))
                {
                    self.state = PlaybackState::Paused;
                    self.publish(self.generation);
                }
            }
            let playing = matches!(self.state, PlaybackState::Playing)
                || (self.effects_held && !self.intended_paused);
            let ready = (!self.effects_held || self.effects_pause_confirmed)
                && self.effects.tick(&mut self.backend, playing, false);
            if ready
                && self.effects_held
                && self.effects_pause_confirmed
                && !self.intended_paused
                && self.state == PlaybackState::Paused
                && self.backend.write_property(BackendProperty::Pause(false))
            {
                self.effects_held = false;
                self.state = PlaybackState::Playing;
                self.publish(self.generation);
            }
            // Pump one backend event (or none → brief idle sleep).
            match self.backend.pump() {
                Some(BackendEvent::Shutdown) => return,
                Some(BackendEvent::FileLoaded) => {
                    self.load_completed = true;
                    // A loaded file is the first moment `time-pos`/`duration`
                    // exist, so this is where the actor subscribes. Skipping it
                    // leaves the actor with no property events at all and the
                    // progress bar / lyrics frozen at 0.
                    self.backend.observe_continuous();
                    // Re-assert the load's pause intent now that the flag
                    // provably belongs to *this* file. mpv keeps a runtime
                    // `pause` change across `loadfile`, so without this the
                    // `pause=yes` a 冷启动 prime wrote silences every later
                    // 点歌 (the clock never moves, yet the snapshot says
                    // `Playing`), while a play-intent load can inherit a stale
                    // ON flag. The load's intent — not the fact that a file
                    // loaded — is what mpv must end up respecting.
                    let ready = self
                        .effects
                        .tick(&mut self.backend, !self.intended_paused, true);
                    self.effects_held = !ready && !self.intended_paused;
                    let pause_applied = self
                        .backend
                        .write_property(BackendProperty::Pause(self.intended_paused || !ready));
                    self.effects_pause_confirmed = pause_applied && !ready;
                    if !pause_applied {
                        tracing::warn!(
                            paused = self.intended_paused,
                            "mpv actor: failed to apply the load's pause intent"
                        );
                    }
                    if let Some(position) = self.pending_seek.take() {
                        if self.backend.write_property(BackendProperty::Seek(position)) {
                            self.position = Some(position);
                        } else {
                            tracing::warn!(
                                position,
                                "mpv actor: failed to restore playback position"
                            );
                        }
                    }
                    // Keep the last authoritative state when mpv rejected the
                    // write. In particular Loading cannot claim a silent hold.
                    if pause_applied {
                        self.state = if self.intended_paused || !ready {
                            PlaybackState::Paused
                        } else {
                            PlaybackState::Playing
                        };
                    }
                    self.publish(self.generation);
                }
                Some(BackendEvent::AudioEffectsFailed) => {
                    self.effects.configuration_failed();
                }
                Some(BackendEvent::AudioReconfigured) => {
                    let affected = self.effects.reconfigured(&mut self.backend);
                    if affected
                        && self.effects.requested()
                        && !self.intended_paused
                        && matches!(
                            self.state,
                            PlaybackState::Loading | PlaybackState::Playing | PlaybackState::Paused
                        )
                    {
                        self.effects_held = true;
                        self.effects_pause_confirmed =
                            self.backend.write_property(BackendProperty::Pause(true));
                        if self.effects_pause_confirmed && self.state == PlaybackState::Playing {
                            self.state = PlaybackState::Paused;
                            self.publish(self.generation);
                        }
                    }
                }
                Some(BackendEvent::Ended) => {
                    self.effects_held = false;
                    self.state = PlaybackState::Ended;
                    self.position = None;
                    self.publish(self.generation);
                }
                Some(BackendEvent::PropertyChanged { name, value }) => {
                    if self.effects.property(&name, value) {
                        continue;
                    }
                    // A discrete control change (transport / mute) must reach the
                    // UI at once; the continuous position/duration stream stays
                    // throttled so a 10-minute track does not push 10 Hz of
                    // snapshots through the IPC bridge.
                    let mut discrete = false;
                    match name.as_str() {
                        "time-pos" => self.position = Some(value),
                        "duration" => self.duration = Some(value),
                        // mpv reports volume in 0.0–100.0; we keep 0.0–1.0.
                        "volume" => {
                            let volume = (value / 100.0).clamp(0.0, 1.0);
                            if (self.volume - volume).abs() > VOLUME_EPSILON {
                                self.volume = volume;
                            }
                            if self.volume > 0.0 {
                                self.last_nonzero_volume = self.volume;
                            }
                        }
                        "mute" => {
                            // mpv reports mute as 0.0 (off) or 1.0 (on) here.
                            // This is what ends the reported 静音分叉: the actor
                            // used to flip its own `muted` on a successful write
                            // and never ask mpv, so a write that did not land
                            // left the UI saying 未静音 while the output stayed
                            // muted — and the next 调大音量 hit exactly that.
                            let muted = value != 0.0;
                            if self.muted != muted {
                                self.muted = muted;
                                discrete = true;
                            }
                        }
                        "pause" => {
                            if self.effects_held {
                                self.effects_pause_confirmed = value != 0.0;
                                continue;
                            }
                            // mpv's flag is the single source of truth for the
                            // transport. `intended_paused` is deliberately *not*
                            // touched here — that field is what the next load
                            // must reach (a command's intent); this is what mpv
                            // actually did.
                            if let Some(next) = Self::transport_from_pause(self.state, value != 0.0)
                            {
                                self.state = next;
                                discrete = true;
                            }
                        }
                        _ => {}
                    }
                    if discrete {
                        self.publish(self.generation);
                    } else {
                        self.publish_throttled(self.generation);
                    }
                }
                None => {
                    // No event: brief sleep to bound CPU. The real backend
                    // already blocks in wait_event briefly; this keeps the
                    // test backend from spinning when its events drain.
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }

    /// Handle a command; returns false to exit the loop (Shutdown).
    #[allow(clippy::too_many_lines)] // Command variants share one state-transition boundary; task 5 splits the actor module.
    fn handle_command(&mut self, cmd: PlayerCommand) -> bool {
        match cmd {
            PlayerCommand::Shutdown => return false,
            PlayerCommand::LoadLibrarySong {
                song_id,
                session_id: _session,
            } => {
                // Resolve the SongId → absolute path on the actor thread and
                // load it. A missing resolver or an unknown/unbound song goes
                // to `Failed` (never a broken `Playing`); the coordinator's
                // error-skip then advances past it.
                //
                // 点歌是「播放」意图: the intent is re-asserted on FileLoaded,
                // because a prior primed load left mpv with `pause=yes` and mpv
                // does not clear that on a new file (see `intended_paused`).
                self.intended_paused = false;
                self.begin_load();
                if let Some(path) = self.resolver.as_ref().and_then(|r| (r)(song_id).ok()) {
                    self.load_path(&path);
                } else {
                    self.state = PlaybackState::Failed;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::LoadLibrarySongPaused {
                song_id,
                session_id: _session,
            } => {
                // Same resolution rules as `LoadLibrarySong`, but the backend
                // is told to hold the file paused: the load completes (real
                // duration + cover) while mpv's pause flag guarantees no sound
                // until the user presses 播放.
                self.intended_paused = true;
                self.begin_load();
                if let Some(path) = self.resolver.as_ref().and_then(|r| (r)(song_id).ok()) {
                    self.load_path(&path);
                    // Silence the window between `loadfile` and FileLoaded
                    // as well: the flag is written now and re-asserted on
                    // FileLoaded, so neither an idle mpv nor the new file
                    // ever gets a chance to sound.
                    if !self.backend.write_property(BackendProperty::Pause(true)) {
                        tracing::warn!("mpv actor: failed to set pause for paused load");
                    }
                } else {
                    self.state = PlaybackState::Failed;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::LoadTemporary {
                display_name: _,
                path,
                session_id: _,
            } => {
                self.intended_paused = false;
                self.begin_load();
                self.load_path(&path);
            }
            PlayerCommand::Play => {
                // The write is the point: previously this only flipped the
                // actor's own state flag, so 播放/暂停 changed the icon while the
                // audio kept running.
                self.intended_paused = false;
                let ready = self.effects.tick(&mut self.backend, true, true);
                self.effects_held = !ready;
                let pause_applied = self.backend.write_property(BackendProperty::Pause(!ready));
                self.effects_pause_confirmed = pause_applied && !ready;
                if pause_applied && self.state.can_transition_to(PlaybackState::Playing) {
                    self.state = if ready {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Paused
                    };
                    self.publish(self.generation);
                }
            }
            PlayerCommand::Pause => {
                self.intended_paused = true;
                if self.backend.write_property(BackendProperty::Pause(true))
                    && (self.state == PlaybackState::Paused
                        || self.state.can_transition_to(PlaybackState::Paused))
                {
                    self.effects_held = false;
                    self.state = PlaybackState::Paused;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::TogglePlayPause => {
                let target = match self.state {
                    PlaybackState::Paused if self.effects_held && !self.intended_paused => {
                        Some(PlaybackState::Paused)
                    }
                    PlaybackState::Playing => Some(PlaybackState::Paused),
                    PlaybackState::Paused => Some(PlaybackState::Playing),
                    _ => None,
                };
                if let Some(target) = target {
                    let pause = target == PlaybackState::Paused;
                    let ready = pause || self.effects.tick(&mut self.backend, true, true);
                    self.effects_held = !ready;
                    let pause_applied = self
                        .backend
                        .write_property(BackendProperty::Pause(pause || !ready));
                    self.effects_pause_confirmed = pause_applied && !ready;
                    if pause_applied {
                        // The user's latest transport choice is the intent a
                        // pending load must not override when it lands.
                        self.intended_paused = pause;
                        self.state = if ready { target } else { PlaybackState::Paused };
                        self.publish(self.generation);
                    }
                }
            }
            PlayerCommand::Next | PlayerCommand::Previous => {
                // The coordinator advances the queue and issues a new load; mpv
                // reaches natural EOF and reports Ended. We surface Ended so
                // the coordinator knows to advance.
                self.effects_held = false;
                self.state = PlaybackState::Ended;
                self.position = None;
                self.publish(self.generation);
            }
            PlayerCommand::Seek(pos) => {
                if self.state == PlaybackState::Loading {
                    self.pending_seek = Some(pos);
                    return true;
                }
                // Seek publishes immediately on success; on backend rejection
                // the snapshot is untouched (authoritative rollback, task 8.8).
                if self.backend.write_property(BackendProperty::Seek(pos)) {
                    self.position = Some(pos);
                    self.publish(self.generation);
                }
            }
            PlayerCommand::SetMode(mode) => {
                self.mode = mode;
                self.publish(self.generation);
            }
            PlayerCommand::SetVolume(vol) => {
                let clamped = vol.clamp(0.0, 1.0);
                // A clicked/Slider-driven SetVolume clears mute (the user is
                // choosing a loudness) and re-members the non-zero value.
                if clamped > 0.0 {
                    self.last_nonzero_volume = clamped;
                }
                // …and clearing mute has to reach mpv, not just this struct:
                // `volume` and `mute` are *independent* properties in mpv, so
                // `set volume` leaves `mute=yes` untouched. Writing only the
                // volume made the snapshot claim 未静音 while the output stayed
                // muted — the UI showed sound and the speaker had none, and the
                // next 调大音量 landed right on it (this defect family's audio
                // form). Both writes must land for the change to be committed.
                let unmute = self.muted && clamped > 0.0;
                let volume_ok = self
                    .backend
                    .write_property(BackendProperty::Volume(clamped));
                let mute_ok = if unmute {
                    self.backend.write_property(BackendProperty::Mute(false))
                } else {
                    true
                };
                if volume_ok && mute_ok {
                    self.volume = clamped;
                    if unmute {
                        self.muted = false;
                    }
                    self.publish(self.generation);
                }
            }
            PlayerCommand::ToggleMute => {
                // Toggling off restores the last non-zero volume (task 8.8).
                let target = !self.muted;
                let restore = !target && self.last_nonzero_volume > 0.0;
                let want_mute = target;
                if self
                    .backend
                    .write_property(BackendProperty::Mute(want_mute))
                {
                    self.muted = want_mute;
                    if restore {
                        // Unmute back to the remembered non-zero volume.
                        self.volume = self.last_nonzero_volume;
                    } else if self.muted {
                        // Remember the current audible volume before muting.
                        self.last_nonzero_volume = if self.volume > 0.0 {
                            self.volume
                        } else {
                            self.last_nonzero_volume
                        };
                    }
                    self.publish(self.generation);
                }
            }
            PlayerCommand::SetMute(mute) => {
                // Absolute, idempotent counterpart of `ToggleMute`, used by
                // session restore: the persisted session dictates the output
                // state, so re-applying it must land on the same value every
                // time (a relative toggle would oscillate between restores).
                // Already being at the target state is a no-op, not a rewiring.
                if self.muted != mute {
                    let restore = !mute && self.last_nonzero_volume > 0.0;
                    if self.backend.write_property(BackendProperty::Mute(mute)) {
                        self.muted = mute;
                        if restore {
                            // Unmute back to the remembered non-zero volume.
                            self.volume = self.last_nonzero_volume;
                        } else if self.volume > 0.0 {
                            // Remember the current audible volume before muting.
                            self.last_nonzero_volume = self.volume;
                        }
                        self.publish(self.generation);
                    }
                }
            }
            PlayerCommand::SetForeground(fg) => {
                // Changing the throttle rate never forces a publish by itself;
                // the next property event adopts the new interval. A pause /
                // track change still publishes immediately regardless.
                self.foreground = fg;
                // Reset the throttle basis so the new rate applies promptly.
                self.last_property_publish = std::time::Instant::now();
            }
            PlayerCommand::Stop => {
                // Stop must actually silence the backend, not only move the
                // actor's state flag — mpv would otherwise keep the file open
                // (and audible) after a coordinated delete's unload barrier.
                // A full unload (releasing the fd) is a Windows-file-lock
                // concern; on macOS unlink works on an open file and pausing
                // is the honest transport stop.
                let _ = self.backend.write_property(BackendProperty::Pause(true));
                self.intended_paused = true;
                self.state = PlaybackState::Stopped;
                self.position = None;
                self.publish(self.generation);
            }
        }
        true
    }

    /// Begin one load attempt: bump the generation and clear the stale
    /// progress facts (normalize-os-file-open-paths). Every load entry point —
    /// the two library-song resolver branches and `LoadTemporary` — MUST call
    /// this first, so a `Loading` or `Failed` snapshot never carries the
    /// `position`/`duration` left over from the previously loaded file.
    fn begin_load(&mut self) -> u64 {
        self.effects_pause_confirmed = self.effects.begin_load(&mut self.backend);
        self.load_completed = false;
        self.effects_held = self.effects.requested();
        self.generation += 1;
        self.pending_seek = None;
        self.position = None;
        self.duration = None;
        self.generation
    }

    fn load_path(&mut self, path: &Path) {
        // Defense-in-depth (task 8.3): only Rust-validated LOCAL paths reach
        // mpv. Refuse anything that looks like a URL/scheme (`http://`, `smb://`,
        // `mms://`, …) so a caller can never smuggle a networked protocol in,
        // even though `protocol-whitelist=file` would already block it.
        if !is_local_media_path(path) {
            tracing::warn!(
                ?path,
                "mpv actor: refusing non-local media path (URL scheme present)"
            );
            self.state = PlaybackState::Failed;
            self.publish(self.generation);
            return;
        }
        self.state = PlaybackState::Loading;
        self.publish(self.generation);
        self.backend.queue_load(path);
    }
}

/// Whether `path` is acceptable to hand to mpv: local and free of a URL scheme.
///
/// A path containing `://` (e.g. `http://…`, `smb://…`, `mms://…`) is a
/// protocol specifier, not a local file path. On macOS/Windows/Linux a real
/// file path never contains `://`, so this cheap check closes the last gap
/// between the actor and mpv's own `protocol-whitelist`.
#[must_use]
fn is_local_media_path(path: &Path) -> bool {
    !path.to_string_lossy().contains("://")
}

/// Drain commands until `Shutdown` when no backend is available (degraded
/// start); keeps the bounded channel a straightforward contract.
fn drain_without_backend(
    rx: mpsc::Receiver<PlayerCommand>,
    snapshot: &Arc<RwLock<PlayerSnapshot>>,
    stopped: &Arc<AtomicBool>,
) {
    for cmd in rx {
        if matches!(cmd, PlayerCommand::Shutdown) {
            break;
        }
    }
    *snapshot.write().expect("actor snapshot poisoned") = PlayerSnapshot::default();
    stopped.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "native_confirmation_tests.rs"]
mod native_confirmation_tests;
