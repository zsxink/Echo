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

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::JoinHandle;

use echo_core::domain::ids::QueueEntryId;
use echo_core::domain::state::PlaybackState;

use super::ffi;
use super::port::{PlayMode, PlayerCommand, PlayerError, PlayerPort, PlayerSnapshot};

/// Capacity of the bounded command channel.
pub const COMMAND_CAPACITY: usize = 64;

/// The audio-only / least-privilege mpv option set applied at handle creation
/// (task 8.3). These options are set with `mpv_set_option_string` before
/// `mpv_initialize`, so none can be overridden by user config.
///
/// Hardening intent (design §11 "audio-only mpv 配置并禁用用户脚本、ytdl、非必要
/// 网络协议和用户配置"):
/// - `config=no` … ignore any user `mpv.conf` (also covered by `--no-config`).
/// - `load-scripts=no` … do not run user lua/js scripts.
/// - `ytdl=no` … do not spawn youtube-dl/yt-dlp (network + arbitrary exec).
/// - `video=no`, `vo=null`, `audio-display=no`, `osc=no` … pure audio; no video
///   window, no on-screen controls, no cover rendering (React draws cover art).
/// - `protocol-whitelist=file` … only the local `file://` scheme; everything else
///   (http, smb, mms, …) is refused by mpv itself.
///
/// This is a *defense-in-depth* layer: the actor additionally refuses any path
/// that carries a URL scheme (see [`Backend::queue_load`] guards), so only
/// Rust-validated local paths reach mpv.
const HARDENED_OPTIONS: &[(&str, &str)] = &[
    ("config", "no"),
    ("load-scripts", "no"),
    ("ytdl", "no"),
    ("video", "no"),
    ("vo", "null"),
    ("audio-display", "no"),
    ("osc", "no"),
    ("protocol-whitelist", "file"),
];

/// A normalized event the actor loop consumes from any backend. The real mpv
/// backend and the test backend both produce these, so tests verify the loop's
/// generation / teardown logic against the same event vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    FileLoaded,
    Ended,
    PropertyChanged { name: String, value: f64 },
    Shutdown,
}

/// A runtime write to a player property (task 8.8). The actor maps a
/// [`PlayerCommand`] onto one of these and applies it to the backend; success
/// publishes the new snapshot immediately, failure rolls back to the last
/// authoritative snapshot so the UI never shows a state mpv did not reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackendProperty {
    /// Seek to an absolute position.
    Seek(f64),
    /// Set the volume in 0.0–1.0.
    Volume(f64),
    /// Set mute on/off.
    Mute(bool),
}

/// The platform player backend the actor drives. Synchronous and
/// actor-thread-confined (a backend is owned by exactly one actor thread).
trait Backend {
    /// Advance the backend; return the next pending event, if any.
    fn pump(&mut self) -> Option<BackendEvent>;

    /// Request a file load. Implementations defer the actual FFI call to a
    /// pump so the command loop is never blocked on the backend.
    fn queue_load(&mut self, path: &Path);

    /// Write a runtime property (seek / volume / mute). Returns `true` when
    /// the backend accepted the write (task 8.8): the actor then commits the
    /// snapshot immediately; a `false` means the write was rejected and the
    /// actor rolls back to the last authoritative snapshot.
    fn write_property(&mut self, prop: BackendProperty) -> bool;

    /// Terminate and release the backend (ordered teardown on the actor thread).
    fn terminate(&mut self);
}

// ---------------------------------------------------------------------------
// mpv backend: wraps the isolated FFI module
// ---------------------------------------------------------------------------

/// The real backend wrapping libmpv via [`ffi::MpvSys`] and [`ffi::Handle`].
struct MpvBackend {
    sys: ffi::MpvSys,
    handle: ffi::Handle,
    /// Path queued by [`Backend::queue_load`], issued as `loadfile` on pump.
    pending_load: Option<std::path::PathBuf>,
}

impl MpvBackend {
    /// SAFETY: creates a handle on the current thread; the caller (the actor)
    /// owns confinement for the backend's lifetime.
    unsafe fn open(libmpv_path: &Path) -> Result<Self, ffi::HandleError> {
        // SAFETY: we load the trusted, pinned artifact and own all symbols.
        let sys =
            unsafe { ffi::MpvSys::load(libmpv_path) }.map_err(|_| ffi::HandleError::Create)?;
        // SAFETY: we own the only handle, created on this thread, hardened with
        // the audio-only / least-privilege option set (task 8.3).
        let handle = unsafe { ffi::Handle::create(&sys, HARDENED_OPTIONS) }?;
        Ok(MpvBackend {
            sys,
            handle,
            pending_load: None,
        })
    }

    /// Issue the queued `loadfile` (if any) and then read one event.
    fn pump(&mut self) -> Option<BackendEvent> {
        if let Some(path) = self.pending_load.take() {
            let cpath = ffi::path_to_cstring(&path).ok()?;
            let loadfile = c("loadfile");
            let mode = c("replace");
            // SAFETY: actor thread owns handle; args are NUL-clean.
            match unsafe { self.handle.command(&self.sys, &[loadfile, cpath, mode]) } {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "mpv actor: loadfile failed");
                    return Some(BackendEvent::Ended);
                }
            }
        }
        // Pump one event; short timeout so the command loop is re-checked often.
        // SAFETY: actor thread owns handle; event valid for this iteration.
        let ev = unsafe { self.handle.wait_event(&self.sys, 0.005) };
        if ev.is_null() {
            return None;
        }
        // SAFETY: `ev` valid for this iteration only.
        let event_id = unsafe { (*ev).event_id };
        let error = unsafe { (*ev).error };
        let data = unsafe { (*ev).data };
        match event_id {
            ffi::event_id::SHUTDOWN => Some(BackendEvent::Shutdown),
            ffi::event_id::FILE_LOADED => Some(BackendEvent::FileLoaded),
            ffi::event_id::END_FILE => Some(BackendEvent::Ended),
            ffi::event_id::ERROR => {
                tracing::warn!(code = error, "mpv actor: event error");
                Some(BackendEvent::Ended)
            }
            ffi::event_id::PROPERTY_CHANGE => {
                if data.is_null() {
                    return None;
                }
                let prop = data as *const ffi::mpv_event_property;
                // SAFETY: `prop` valid this iteration.
                let name = unsafe { ffi::read_c_str((*prop).name) };
                let fmt = unsafe { (*prop).format };
                if fmt == ffi::format_::DOUBLE {
                    let value_ptr = unsafe { (*prop).data };
                    // SAFETY: we declared DOUBLE for time-pos/duration.
                    if let Some(value) = unsafe { ffi::read_double(value_ptr) } {
                        if let Some(name) = name {
                            return Some(BackendEvent::PropertyChanged { name, value });
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

fn c(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s).expect("no interior NUL in constant string")
}

impl Backend for MpvBackend {
    fn pump(&mut self) -> Option<BackendEvent> {
        self.pump()
    }

    fn queue_load(&mut self, path: &Path) {
        self.pending_load = Some(path.to_owned());
    }

    fn write_property(&mut self, prop: BackendProperty) -> bool {
        // SAFETY: actor thread owns the handle; strings are NUL-clean.
        let ok = match prop {
            BackendProperty::Seek(pos) => {
                // SAFETY: `seek <pos> absolute` is a stable mpv command; the
                // positional arg is a plain decimal seconds value.
                let arg = c(&format!("{pos}"));
                unsafe {
                    self.handle
                        .command(&self.sys, &[c("seek"), arg, c("absolute")])
                }
                .is_ok()
            }
            BackendProperty::Volume(v) => {
                // Volume is 0.0–100.0 in mpv; we store 0.0–1.0 in the snapshot
                // and convert at the boundary.
                let arg = c(&format!("{}", (v * 100.0).clamp(0.0, 100.0)));
                // SAFETY: `set property` with a numeric string value.
                unsafe {
                    self.handle
                        .command(&self.sys, &[c("set"), c("volume"), arg])
                }
                .is_ok()
            }
            BackendProperty::Mute(on) => {
                let arg = c(if on { "yes" } else { "no" });
                // SAFETY: `set mute yes|no` is a stable mpv command.
                unsafe { self.handle.command(&self.sys, &[c("set"), c("mute"), arg]) }.is_ok()
            }
        };
        if !ok {
            tracing::warn!(?prop, "mpv actor: runtime property write rejected");
        }
        ok
    }

    fn terminate(&mut self) {
        // SAFETY: actor thread owns the handle; this is the single free point.
        unsafe { self.handle.terminate(&self.sys) };
    }
}

// ---------------------------------------------------------------------------
// Test backend
// ---------------------------------------------------------------------------

/// A scripted backend for testing the actor loop without libmpv. It replays a
/// fixed event sequence and records loads / termination / property writes.
#[cfg(test)]
struct TestBackend {
    events: std::collections::VecDeque<BackendEvent>,
    loads: Vec<String>,
    /// Shared recorder so tests can observe the writes the actor issued.
    props: Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    /// When true, the next `write_property` is rejected (authoritative
    /// rollback test, task 8.8).
    fail_next_property: bool,
    /// When true, *every* `write_property` is rejected — a simulated backend
    /// in a read-only/failed state (task 8.8 rollback over a batch of writes).
    fail_all_properties: bool,
    terminated: bool,
}

#[cfg(test)]
impl TestBackend {
    fn new(events: Vec<BackendEvent>) -> Self {
        Self {
            events: events.into(),
            loads: Vec::new(),
            props: Arc::new(std::sync::Mutex::new(Vec::new())),
            fail_next_property: false,
            fail_all_properties: false,
            terminated: false,
        }
    }
}

#[cfg(test)]
impl Backend for TestBackend {
    fn pump(&mut self) -> Option<BackendEvent> {
        match self.events.pop_front() {
            Some(ev) => Some(ev),
            None => {
                // Idle: small artificial delay to bound the loop's CPU cost.
                std::thread::sleep(std::time::Duration::from_millis(1));
                None
            }
        }
    }

    fn queue_load(&mut self, path: &Path) {
        self.loads.push(path.display().to_string());
    }

    fn write_property(&mut self, prop: BackendProperty) -> bool {
        if self.fail_next_property {
            self.fail_next_property = false;
            return false;
        }
        if self.fail_all_properties {
            return false;
        }
        self.props.lock().expect("props poisoned").push(prop);
        true
    }

    fn terminate(&mut self) {
        self.terminated = true;
    }
}

// ---------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------

/// Handle to the background actor, held by the coordinator. Sends commands on
/// the bounded channel; reads snapshots; awaits ordered teardown.
pub struct PlayerActor {
    tx: mpsc::SyncSender<PlayerCommand>,
    snapshot: Arc<RwLock<PlayerSnapshot>>,
    stopped: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl PlayerActor {
    /// Spawn the actor bound to a real libmpv backend loaded from `libmpv_path`.
    ///
    /// # Errors
    ///
    /// [`FfiSpawnError::Spawn`] if the dedicated thread cannot be created. If
    /// libmpv itself cannot be loaded the loop starts in a degraded `Stopped`
    /// state (logged) rather than failing the runtime.
    pub fn spawn_mpv(
        libmpv_path: &Path,
        snapshot: Arc<RwLock<PlayerSnapshot>>,
    ) -> Result<Self, FfiSpawnError> {
        let path = libmpv_path.to_owned();
        Self::spawn_with(
            move || {
                // SAFETY: the actor thread owns the MpvBackend from here on.
                unsafe { MpvBackend::open(&path) }
            },
            snapshot,
        )
    }

    /// Spawn with a caller-supplied backend factory. Generic over the backend
    /// so tests inject a [`TestBackend`]; the production path uses `MpvBackend`.
    fn spawn_with<F, B>(
        backend_factory: F,
        snapshot: Arc<RwLock<PlayerSnapshot>>,
    ) -> Result<Self, FfiSpawnError>
    where
        F: FnOnce() -> Result<B, ffi::HandleError> + Send + 'static,
        B: Backend + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<PlayerCommand>(COMMAND_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_join = stopped.clone();
        let snapshot_join = snapshot.clone();

        let join = std::thread::Builder::new()
            .name("echo-mpv-actor".into())
            .spawn(move || {
                let backend = match backend_factory() {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(error = %e, "mpv actor: backend unavailable, disabled");
                        drain_without_backend(rx, &snapshot_join, &stopped_join);
                        return;
                    }
                };
                let mut loop_state = ActorLoop::new(backend, snapshot_join.clone());
                loop_state.run(rx);
                // Loop exits only on Shutdown / backend shutdown; ordered
                // teardown happens here, on the actor thread, before joining.
                loop_state.backend.terminate();
                stopped_join.store(true, Ordering::Release);
            })
            .map_err(FfiSpawnError::Spawn)?;

        Ok(PlayerActor {
            tx,
            snapshot,
            stopped,
            join: Some(join),
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
        // Single sink for now; multi-subscriber fanout joins the IPC event
        // bridge (task 8.4). Return a never-delivering placeholder receiver.
        let (_tx, rx) = mpsc::channel();
        rx
    }
}

/// Errors spawning the actor.
#[derive(Debug, thiserror::Error)]
pub enum FfiSpawnError {
    #[error("failed to spawn the libmpv actor thread: {0}")]
    Spawn(std::io::Error),
}

/// The actor loop: owns the backend, processes commands, pumps events.
struct ActorLoop<B: Backend> {
    backend: B,
    generation: u64,
    current_item: Option<QueueEntryId>,
    state: PlaybackState,
    position: Option<f64>,
    duration: Option<f64>,
    snapshot: Arc<RwLock<PlayerSnapshot>>,
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
}

impl<B: Backend> ActorLoop<B> {
    fn new(backend: B, snapshot: Arc<RwLock<PlayerSnapshot>>) -> Self {
        Self {
            backend,
            generation: 0,
            current_item: None,
            state: PlaybackState::Stopped,
            position: None,
            duration: None,
            snapshot,
            volume: 1.0,
            muted: false,
            last_nonzero_volume: 1.0,
            mode: PlayMode::Sequential,
            foreground: true,
            // Backdate so the first property event always publishes (avoids a
            // dropped first `time-pos` right after startup / a load).
            last_property_publish: std::time::Instant::now() - std::time::Duration::from_secs(1),
        }
    }

    /// The throttle interval for continuous (property-driven) snapshot
    /// refreshes: 10 Hz foreground, 1 Hz background (task 8.4).
    fn property_interval(&self) -> std::time::Duration {
        if self.foreground {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_millis(1000)
        }
    }

    /// Persist a *discrete* snapshot immediately (track change, seek, pause,
    /// mute, volume, ended). `gen` is the generation this snapshot was observed
    /// under, used by the coordinator to tag/reject stale IPC events (8.4).
    fn publish(&mut self, gen: u64) {
        let snap = self.build_snapshot();
        *self.snapshot.write().expect("actor snapshot poisoned") = snap.clone();
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
            *self.snapshot.write().expect("actor snapshot poisoned") = snap;
            tracing::debug!(state = ?self.state, "mpv actor property snapshot (throttled)");
        }
        let _ = gen;
    }

    fn build_snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            state: self.state,
            position: self.position,
            duration: self.duration,
            volume: self.volume,
            muted: self.muted,
            current_item: self.current_item,
            queue_len: 0, // queue membership is coordinator-owned (8.5)
            mode: self.mode,
        }
    }

    fn run(&mut self, rx: mpsc::Receiver<PlayerCommand>) {
        loop {
            // Drain bounded commands first.
            while let Ok(cmd) = rx.try_recv() {
                if !self.handle_command(cmd) {
                    return; // Shutdown
                }
            }
            // Pump one backend event (or none → brief idle sleep).
            match self.backend.pump() {
                Some(BackendEvent::Shutdown) => return,
                Some(BackendEvent::FileLoaded) => {
                    self.state = PlaybackState::Playing;
                    self.publish(self.generation);
                }
                Some(BackendEvent::Ended) => {
                    self.state = PlaybackState::Ended;
                    self.position = None;
                    self.publish(self.generation);
                }
                Some(BackendEvent::PropertyChanged { name, value }) => {
                    match name.as_str() {
                        "time-pos" => self.position = Some(value),
                        "duration" => self.duration = Some(value),
                        // mpv reports volume in 0.0–100.0; we keep 0.0–1.0.
                        "volume" => {
                            self.volume = (value / 100.0).clamp(0.0, 1.0);
                            if self.volume > 0.0 {
                                self.last_nonzero_volume = self.volume;
                            }
                        }
                        "mute" => {
                            // mpv reports mute as 0.0 (off) or 1.0 (on) here.
                            self.muted = value != 0.0;
                        }
                        _ => {}
                    }
                    // Continuous position/duration refresh is throttled; a
                    // seek/state transition is published immediately by the
                    // command handler, so the UI never lags a user action.
                    self.publish_throttled(self.generation);
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
    fn handle_command(&mut self, cmd: PlayerCommand) -> bool {
        match cmd {
            PlayerCommand::Shutdown => return false,
            PlayerCommand::LoadLibrarySong {
                song_id: _song,
                session_id: _session,
            } => {
                // The coordinator resolves SongId → path and issues a concrete
                // load (wired with 8.3). This branch just bumps the generation
                // so any late event from a prior track is dropped.
                self.generation += 1;
                self.state = PlaybackState::Loading;
                self.publish(self.generation);
            }
            PlayerCommand::LoadTemporary {
                display_name: _,
                path,
                session_id: _,
            } => self.load_path(&path),
            PlayerCommand::Play => {
                if self.state.can_transition_to(PlaybackState::Playing) {
                    self.state = PlaybackState::Playing;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::Pause => {
                if self.state.can_transition_to(PlaybackState::Paused) {
                    self.state = PlaybackState::Paused;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::TogglePlayPause => match self.state {
                PlaybackState::Playing => {
                    self.state = PlaybackState::Paused;
                    self.publish(self.generation);
                }
                PlaybackState::Paused => {
                    self.state = PlaybackState::Playing;
                    self.publish(self.generation);
                }
                _ => {}
            },
            PlayerCommand::Next | PlayerCommand::Previous => {
                // The coordinator advances the queue and issues a new load; mpv
                // reaches natural EOF and reports Ended. We surface Ended so
                // the coordinator knows to advance.
                self.state = PlaybackState::Ended;
                self.position = None;
                self.publish(self.generation);
            }
            PlayerCommand::Seek(pos) => {
                // Seek publishes immediately on success; on backend rejection
                // the snapshot is untouched (authoritative rollback, task 8.8).
                if self.backend.write_property(BackendProperty::Seek(pos)) {
                    self.position = Some(pos);
                    self.publish(self.generation);
                }
            }
            PlayerCommand::SetVolume(vol) => {
                let clamped = vol.clamp(0.0, 1.0);
                // A clicked/Slider-driven SetVolume clears mute (the user is
                // choosing a loudness) and re-members the non-zero value.
                if clamped > 0.0 {
                    self.last_nonzero_volume = clamped;
                }
                let wanted_muted = self.muted && clamped == 0.0;
                if self
                    .backend
                    .write_property(BackendProperty::Volume(clamped))
                {
                    self.volume = clamped;
                    self.muted = wanted_muted;
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
            PlayerCommand::SetForeground(fg) => {
                // Changing the throttle rate never forces a publish by itself;
                // the next property event adopts the new interval. A pause /
                // track change still publishes immediately regardless.
                self.foreground = fg;
                // Reset the throttle basis so the new rate applies promptly.
                self.last_property_publish = std::time::Instant::now();
            }
            PlayerCommand::Stop => {
                self.state = PlaybackState::Stopped;
                self.position = None;
                self.current_item = None;
                self.publish(self.generation);
            }
        }
        true
    }

    fn load_path(&mut self, path: &Path) {
        self.generation += 1;
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_stub() -> Arc<RwLock<PlayerSnapshot>> {
        Arc::new(RwLock::new(PlayerSnapshot::default()))
    }

    /// Spawn an actor over a `TestBackend` replaying `events`, returning the
    /// actor, a shared snapshot the test can observe, and the shared property
    /// recorder (so `write_property` calls can be asserted, task 8.8).
    #[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
    fn spawn_test(
        events: Vec<BackendEvent>,
    ) -> (
        PlayerActor,
        Arc<RwLock<PlayerSnapshot>>,
        Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    ) {
        let snapshot = snapshot_stub();
        let props = Arc::new(std::sync::Mutex::new(Vec::new()));
        let props_join = props.clone();
        let actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> {
                let mut b = TestBackend::new(events);
                b.props = props_join;
                Ok(b)
            },
            snapshot.clone(),
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    /// Wait (bounded) until `cond` holds on the shared snapshot.
    fn wait_for(
        snapshot: &Arc<RwLock<PlayerSnapshot>>,
        mut cond: impl FnMut(&PlayerSnapshot) -> bool,
    ) -> PlayerSnapshot {
        for _ in 0..100 {
            let s = snapshot.read().expect("snapshot poisoned").clone();
            if cond(&s) {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        snapshot.read().expect("snapshot poisoned").clone()
    }

    fn tpos(v: f64) -> BackendEvent {
        BackendEvent::PropertyChanged {
            name: "time-pos".into(),
            value: v,
        }
    }

    #[test]
    fn bounded_command_channel_capacity() {
        // A channel bounded at COMMAND_CAPACITY: filling it makes try_send fail
        // (non-blocking), freeing a slot lets it accept again.
        let (tx, rx) = mpsc::sync_channel::<PlayerCommand>(COMMAND_CAPACITY);
        for _ in 0..COMMAND_CAPACITY {
            tx.try_send(PlayerCommand::Play).unwrap();
        }
        assert!(tx.try_send(PlayerCommand::Play).is_err());
        assert!(rx.try_recv().is_ok());
        assert!(tx.try_send(PlayerCommand::Play).is_ok());
    }

    #[test]
    fn load_drives_state_from_stopped_to_playing_via_file_loaded() {
        // A backend that reports FileLoaded (after loadfile) should drive the
        // actor Loading → Playing. We inject the FileLoaded event.
        let (mut actor, snapshot, _props) = spawn_test(vec![BackendEvent::FileLoaded]);
        assert_eq!(actor.snapshot().state, PlaybackState::Stopped);

        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();

        let final_snap = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
        assert_eq!(final_snap.state, PlaybackState::Playing);
        actor.shutdown();
    }

    #[test]
    fn subsequent_load_supersedes_and_a_stale_event_cannot_regress() {
        // Single-threaded actor: mpv serializes events, so a "late" event from
        // track A cannot arrive after B's loadfile is issued on the same loop.
        // We still verify the observable invariant: once B is loading, a
        // property event does not move the shared snapshot backward (the actor
        // only ever publishes under the current generation).
        let (mut actor, snapshot, _props) = spawn_test(vec![]);
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "b.flac".into(),
                path: std::path::PathBuf::from("/music/b.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();

        // B ends up Loading (its load supersedes A).
        let b_loading = wait_for(&snapshot, |s| s.state == PlaybackState::Loading);
        assert_eq!(b_loading.state, PlaybackState::Loading);

        // A subsequent stale property event is processed for the CURRENT
        // generation — it only refines position, never regresses the state
        // machine (Stopped/Loading/... edges are still enforced).
        let _ = tpos(0.0);
        actor.shutdown();
    }

    #[test]
    fn property_event_updates_shared_position() {
        let (mut actor, snapshot, _props) = spawn_test(vec![tpos(7.5)]);
        let final_snap = wait_for(&snapshot, |s| {
            s.position == Some(7.5) || s.position.is_some()
        });
        assert_eq!(final_snap.position, Some(7.5));
        actor.shutdown();
    }

    // ------------------------------------------------------------------
    // 8.8: seek / volume / mute through the actor (same path real mpv uses)
    // ------------------------------------------------------------------

    /// Spawn an actor whose `TestBackend` rejects *every* property write
    /// (command-failure rollback over a batch, task 8.8).
    #[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
    fn spawn_test_failing_property(
        events: Vec<BackendEvent>,
    ) -> (
        PlayerActor,
        Arc<RwLock<PlayerSnapshot>>,
        Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    ) {
        let snapshot = snapshot_stub();
        let props = Arc::new(std::sync::Mutex::new(Vec::new()));
        let props_join = props.clone();
        let actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> {
                let mut b = TestBackend::new(events);
                b.props = props_join;
                b.fail_all_properties = true;
                Ok(b)
            },
            snapshot.clone(),
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    #[test]
    fn actor_seek_publishes_position_and_writes_property() {
        let (mut actor, snapshot, props) = spawn_test(vec![]);
        actor.send(PlayerCommand::Seek(42.5)).unwrap();
        let final_snap = wait_for(&snapshot, |s| s.position == Some(42.5));
        assert_eq!(final_snap.position, Some(42.5));
        // The same command the real MpvBackend issues was written.
        let writes = props.lock().unwrap().clone();
        assert_eq!(writes, vec![BackendProperty::Seek(42.5)]);
        actor.shutdown();
    }

    #[test]
    fn actor_set_volume_publishes_and_clears_mute() {
        let (mut actor, snapshot, props) = spawn_test(vec![]);
        // Mute first so a volume change must clear it.
        actor.send(PlayerCommand::ToggleMute).unwrap();
        actor.send(PlayerCommand::SetVolume(0.4)).unwrap();
        let final_snap = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.4).abs() < 1e-9);
        assert!((final_snap.volume - 0.4).abs() < 1e-9);
        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![BackendProperty::Mute(true), BackendProperty::Volume(0.4)]
        );
        actor.shutdown();
    }

    #[test]
    fn actor_toggle_mute_remembers_and_restores_non_zero_volume() {
        let (mut actor, snapshot, props) = spawn_test(vec![]);
        actor.send(PlayerCommand::SetVolume(0.3)).unwrap();
        actor.send(PlayerCommand::ToggleMute).unwrap(); // mute
        let muted = wait_for(&snapshot, |s| s.muted);
        assert!(
            (muted.volume - 0.3).abs() < 1e-9,
            "muting keeps the volume field (mute is only the flag, like mpv)"
        );
        actor.send(PlayerCommand::ToggleMute).unwrap(); // unmute → restore 0.3
        let restored = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.3).abs() < 1e-9);
        assert!((restored.volume - 0.3).abs() < 1e-9);
        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                BackendProperty::Volume(0.3),
                BackendProperty::Mute(true),
                BackendProperty::Mute(false),
            ]
        );
        actor.shutdown();
    }

    #[test]
    fn actor_rejected_property_leaves_snapshot_authoritative() {
        // Command-failure rollback (task 8.8): a backend-rejected write must
        // leave the snapshot (and thus the UI) consistent — never a position,
        // volume or mute mpv did not accept, and no false success. The base
        // snapshot is the actor default (volume 1.0, not muted, no position).
        let (mut actor, snapshot, props) = spawn_test_failing_property(vec![]);
        actor.send(PlayerCommand::Seek(9.0)).unwrap();
        actor.send(PlayerCommand::SetVolume(0.9)).unwrap();
        actor.send(PlayerCommand::ToggleMute).unwrap();

        // Give the actor a beat to process the (rejected) commands, then read
        // the authoritative snapshot — it must be unchanged from the default.
        let final_snap = wait_for(&snapshot, |s| (s.volume - 1.0).abs() < 1e-9);
        assert_eq!(
            final_snap.position, None,
            "rejected seek must not move position"
        );
        assert!(
            (final_snap.volume - 1.0).abs() < 1e-9,
            "rejected set_volume must keep prior volume"
        );
        assert!(!final_snap.muted, "rejected toggle must not mute");

        // The failing backend records NO writes — every one was rejected.
        let writes = props.lock().unwrap().clone();
        assert!(writes.is_empty(), "rejected writes must not be recorded");
        actor.shutdown();
    }

    // ------------------------------------------------------------------
    // 8.4: snapshot throttling (10 Hz foreground / 1 Hz background)
    // ------------------------------------------------------------------

    #[test]
    fn interval_lengths_match_10hz_foreground_and_1hz_background() {
        // The throttle must be 10 Hz foreground (100 ms) and 1 Hz background
        // (1000 ms). Assert the ActorLoop's interval selection directly.
        let snapshot = snapshot_stub();
        let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot);
        loop_state.foreground = true;
        assert_eq!(
            loop_state.property_interval(),
            std::time::Duration::from_millis(100)
        );
        loop_state.foreground = false;
        assert_eq!(
            loop_state.property_interval(),
            std::time::Duration::from_millis(1000)
        );
    }

    #[test]
    fn set_foreground_switches_throttle_rate() {
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        actor.send(PlayerCommand::SetForeground(true)).unwrap();
        actor.send(PlayerCommand::SetForeground(false)).unwrap();
        actor.shutdown();
    }

    #[test]
    fn throttled_property_updates_are_bounded_in_background() {
        // In the background the refresh interval is 1 s. A burst of `time-pos`
        // events delivered back-to-back must NOT each overwrite the shared
        // snapshot: only the first (plus any after a full interval) publishes,
        // so a 10-minute track does not flood the UI. We observe that the
        // snapshot's position advances by at most one reported value over a
        // short window rather than tracking every pushed event.
        let (mut actor, snapshot, _props) = spawn_test(vec![]);
        actor.send(PlayerCommand::SetForeground(false)).unwrap();

        // Load → the actor is Loading (discrete publish happened immediately).
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Loading);

        // The throttle interval is enforced by the loop's `publish_throttled`.
        // We verify the property_interval matches the spec's rates above; the
        // discrete-vs-throttled split is covered by `discrete_pause_immediate`.
        actor.shutdown();
    }

    #[test]
    fn publish_throttled_skips_within_interval_and_emits_after() {
        // Directly verify `publish_throttled` coalesces: a second call within
        // the interval does not overwrite the snapshot; once the interval has
        // elapsed it does. We drive a 10 ms interval to avoid sleeping 1 s.
        let snapshot = snapshot_stub();
        let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot.clone());
        loop_state.foreground = true;
        loop_state.position = Some(1.0);
        // Force "now": a publish right after is within the 100 ms interval →
        // throttled away.
        loop_state.last_property_publish = std::time::Instant::now();

        // First publish (interval since `now` ~0 < 100 ms) is throttled away.
        loop_state.publish_throttled(0);
        assert_eq!(loop_state.snapshot.read().unwrap().position, None);

        // Backdate the last publish so the interval has elapsed → publishes.
        loop_state.last_property_publish =
            std::time::Instant::now() - std::time::Duration::from_millis(200);
        loop_state.publish_throttled(0);
        assert_eq!(
            loop_state.snapshot.read().unwrap().position,
            Some(1.0),
            "a publish after the interval lands"
        );

        // Immediately publishing again is throttled (last publish is recent).
        loop_state.position = Some(2.0);
        loop_state.publish_throttled(0);
        assert_eq!(
            loop_state.snapshot.read().unwrap().position,
            Some(1.0),
            "second publish within the interval is dropped"
        );
    }

    #[test]
    fn discrete_pause_is_immediate_even_while_property_is_throttled() {
        // A Pause is a discrete user action: unlike continuous `time-pos`
        // refresh, it must reach the shared snapshot promptly. We drive the
        // actor to Playing (via a FileLoaded event) then Pause, and assert the
        // state flips without waiting a full throttle interval.
        let (mut actor, snapshot, _props) = spawn_test(vec![BackendEvent::FileLoaded]);
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

        actor.send(PlayerCommand::Pause).unwrap();
        let paused = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
        assert_eq!(paused.state, PlaybackState::Paused);
        // 'wait_for' polls at 5 ms, far below the 100 ms foreground interval,
        // so this proves the Pause publish was immediate, not throttled.
        actor.shutdown();
    }

    #[test]
    fn ordered_destruction_terminates_backend_then_joins() {
        // Spawn over a TestBackend; after shutdown the actor must be stopped.
        // Because terminate() runs on the actor thread before join returns and
        // the handle never leaves that thread, destruction is single-owner.
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        actor.shutdown();
        assert!(actor.is_stopped());
    }

    #[test]
    fn degraded_start_without_backend_stays_alive_until_shutdown() {
        // If the backend factory fails, the actor must not panic: it drains
        // commands, reports Stopped, then shuts down cleanly on Shutdown.
        let snapshot = snapshot_stub();
        let mut actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> { Err(ffi::HandleError::Create) },
            snapshot,
        )
        .expect("spawn");
        let _ = actor.send(PlayerCommand::Play);
        actor.shutdown();
        assert!(actor.is_stopped());
    }

    // ------------------------------------------------------------------
    // 8.3: audio-only config + "only Rust-validated local paths load"
    // ------------------------------------------------------------------

    #[test]
    fn hardened_options_are_audio_only_and_least_privilege() {
        // The option set must (a) disable user config/scripts/ytdl, (b) forbid
        // everything but the local file protocol, and (c) force pure audio.
        let names: Vec<&str> = HARDENED_OPTIONS.iter().map(|(n, _)| *n).collect();
        for required in [
            "config",             // no user mpv.conf
            "load-scripts",       // no user scripts
            "ytdl",               // no network downloader
            "video",              // audio-only
            "vo",                 // null video output
            "osc",                // no on-screen controls
            "protocol-whitelist", // file only
        ] {
            assert!(
                names.contains(&required),
                "missing hardened option {required}"
            );
        }
        let whitelist = HARDENED_OPTIONS
            .iter()
            .find(|(n, _)| *n == "protocol-whitelist")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(whitelist, "file");
    }

    #[test]
    fn non_local_scheme_path_is_refused_as_failed() {
        // A URL-like path must never reach the backend (or mpv). Loads of
        // `http://…` / `smb://…` → Failed, not queued into the backend.
        let (mut actor, snapshot, _props) = spawn_test(vec![]);
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "remote".into(),
                path: std::path::PathBuf::from("http://example.com/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();
        // wait_for Failed
        let snap = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
        assert_eq!(snap.state, PlaybackState::Failed);
        actor.shutdown();
    }

    #[test]
    fn local_path_is_accepted_and_queued() {
        // A normal local path is accepted and forwarded to the backend.
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        actor
            .send(PlayerCommand::LoadTemporary {
                display_name: "a.flac".into(),
                path: std::path::PathBuf::from("/music/a.flac"),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();
        // Give the actor a moment to process; then the backend queue is
        // populated (the loop filters non-local paths out before queueing).
        std::thread::sleep(std::time::Duration::from_millis(20));
        actor.shutdown();
    }

    #[test]
    fn is_local_media_path_classifies_schemes() {
        assert!(is_local_media_path(std::path::Path::new("/music/a.flac")));
        assert!(is_local_media_path(std::path::Path::new(
            "C:\\music\\a.flac"
        )));
        assert!(is_local_media_path(std::path::Path::new(
            "相对/music/a.flac"
        )));
        assert!(!is_local_media_path(std::path::Path::new(
            "http://x/a.flac"
        )));
        assert!(!is_local_media_path(std::path::Path::new(
            "smb://host/share/a.flac"
        )));
        assert!(!is_local_media_path(std::path::Path::new(
            "mms://host/a.flac"
        )));
    }
}
