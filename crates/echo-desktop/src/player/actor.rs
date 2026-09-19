#![allow(unsafe_code)] // Calls into the documented, isolated libmpv FFI adapter.

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
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;
use echo_core::error::Error;

use super::ffi;
use super::port::{
    PlayMode, PlayerCommand, PlayerError, PlayerPort, PlayerSnapshot, VOLUME_EPSILON,
};

/// Capacity of the bounded command channel.
pub const COMMAND_CAPACITY: usize = 64;

/// Resolves a library [`SongId`] to an absolute file path. The resolver is
/// built over the repository + root registry in the composition root and
/// injected into the actor so that `LoadLibrarySong` can resolve and load
/// files on the actor thread (never leaking absolute paths to DTOs).
///
/// Returns `Err` when the song or its root is unknown / unbound.
pub type SongResolver = Arc<dyn Fn(SongId) -> Result<std::path::PathBuf, Error> + Send + Sync>;

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
///
/// **Build-compat split.** The vendor `audio-default` libmpv **(media-kit
/// v0.7.2)** compiles out Lua scripts, yt-dl and the OSC entirely — its
/// `set_option_string` answers `MPV_ERROR_OPTION_NOT_FOUND` for
/// `load-scripts`/`ytdl`/`osc`/`protocol-whitelist`. Those options are *always*
/// desirable hardening, but on such a build the capability is simply absent,
/// which already satisfies the "disable it" intent. They therefore live in
/// [`HARDENED_OPTIONAL`] and are applied best-effort ([`ffi::Handle::create`]
/// skips them with a warning when unsupported). The *required* set below is
/// what the handle must genuinely enforce to be a safe audio-only player.
const HARDENED_REQUIRED: &[(&str, &str)] = &[
    ("config", "no"),
    ("video", "no"),
    ("vo", "null"),
    ("audio-display", "no"),
];

/// Hardening options that target capabilities a reduced build may compile out;
/// applied best-effort (see [`HARDENED_REQUIRED`] comment).
const HARDENED_OPTIONAL: &[(&str, &str)] = &[
    ("load-scripts", "no"),
    ("ytdl", "no"),
    ("osc", "no"),
    ("protocol-whitelist", "file"),
];

/// Optional options extended from `ECHO_MPV_EXTRA_OPTIONS` (`name=value`,
/// semicolon-separated), applied best-effort like [`HARDENED_OPTIONAL`].
///
/// A diagnostics escape hatch: the real-libmpv smoke sets `ao=null` so the
/// event-plumbing assertions stay hermetic (a sandboxed CI runner can stall
/// mpv's core in `CoreAudio` init — `FILE_LOADED` arrives but the playloop never
/// starts and no property/EOF event is ever delivered). It must never carry
/// security-relevant intent: everything here is best-effort.
fn extra_options() -> Vec<(&'static str, &'static str)> {
    let mut opts: Vec<(&'static str, &'static str)> = HARDENED_OPTIONAL.to_vec();
    if let Ok(extra) = std::env::var("ECHO_MPV_EXTRA_OPTIONS") {
        for pair in extra.split(';').filter_map(|s| s.split_once('=')) {
            // Only a small allowlist of names can be injected, so the escape
            // hatch can never re-enable a capability the hardening disabled.
            const ALLOWED: &[&str] = &["ao"];
            if ALLOWED.contains(&pair.0.trim()) {
                // The option table needs 'static strs; the actor spawns once
                // per process, so leaking the value is bounded and deliberate.
                let value: &'static str = Box::leak(pair.1.to_owned().into_boxed_str());
                opts.push(("ao", value));
            }
        }
    }
    opts
}

/// The continuous properties the actor subscribes to with
/// `mpv_observe_property` (DOUBLE format) so the live UI keeps moving.
///
/// `time-pos` drives 播放进度条 and the synced-lyrics highlight; `duration`
/// supplies the progress bar's range. Without these subscriptions the actor
/// receives **no** `PROPERTY_CHANGE` event at all: `position`/`duration` stay
/// `None` forever, so the progress bar sits at 0 and the lyrics never
/// highlight — while the audio plays perfectly (the 播放控制栏 is fed only by
/// snapshots, so a silent subscription reads as a frozen, "dead" bar).
///
/// They are subscribed on every [`BackendEvent::FileLoaded`] rather than once
/// at handle creation because while the player is idle these properties are
/// *unavailable*: `mpv_observe_property` accepts the subscription but every
/// notification then carries `MPV_FORMAT_NONE` (no value) — subscribing to
/// nothing. Measured against the vendored libmpv, not assumed.
///
/// The audio controls are observed as well, and that is what makes the
/// snapshot's `volume` / `muted` / transport state **mpv's own values** rather
/// than the actor's guesses. Before, the actor's `PlayerCommand` handlers
/// flipped their private fields and published them; any write mpv did not
/// honour (or any mpv-side change) left the two diverged — UI said 未静音 while
/// the output was muted, said `Playing` while the clock sat at 0.
///
/// **Each property carries the format it must be observed with.** `mute` and
/// `pause` are `FLAG`s natively: asking for `DOUBLE` is accepted (returns ok)
/// but every notification then arrives as `NONE`, i.e. it silently subscribes
/// to nothing. `volume` / `time-pos` / `duration` are `DOUBLE`s.
const OBSERVED_PROPERTIES: &[(&str, i32)] = &[
    ("time-pos", ffi::format_::DOUBLE),
    ("duration", ffi::format_::DOUBLE),
    ("volume", ffi::format_::DOUBLE),
    ("mute", ffi::format_::FLAG),
    ("pause", ffi::format_::FLAG),
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
    /// Set the pause flag on/off. This is what actually stops and resumes the
    /// audio in mpv; the transport commands must write it or the buttons only
    /// move the actor's own state flag.
    Pause(bool),
}

/// The platform player backend the actor drives. Synchronous and
/// actor-thread-confined (a backend is owned by exactly one actor thread).
trait Backend {
    /// Advance the backend; return the next pending event, if any.
    fn pump(&mut self) -> Option<BackendEvent>;

    /// Request a file load. Implementations defer the actual FFI call to a
    /// pump so the command loop is never blocked on the backend.
    fn queue_load(&mut self, path: &Path);

    /// Subscribe to the properties the UI renders from and the audio controls
    /// the snapshot reports ([`OBSERVED_PROPERTIES`], each with its own
    /// format). Called by the loop on every `FileLoaded` — the first moment the
    /// per-file properties exist — and idempotent (`mpv` replaces the previous
    /// observation for the same name). A backend that subscribes to nothing
    /// starves every snapshot-driven surface of live values.
    fn observe_continuous(&mut self);

    /// Write a runtime property (seek / volume / mute / pause). Returns `true`
    /// when the backend accepted the write (task 8.8): the actor then commits
    /// the snapshot immediately; a `false` means the write was rejected and the
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
        // the audio-only / least-privilege option set (task 8.3). `required`
        // options must hold (config/video/vo); `optional` ones are applied
        // best-effort because the vendor `audio-default` build compiles some
        // capabilities out and reports them unknown.
        let handle = unsafe { ffi::Handle::create(&sys, HARDENED_REQUIRED, &extra_options()) }?;
        Ok(Self {
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
            ffi::event_id::END_FILE => {
                // Only a natural end-of-file (or an error while playing) is a
                // real "track ended". When a new file replaces the current one
                // (`loadfile replace`, i.e. every next/previous/jump), mpv ends
                // the *old* file with reason STOP (or QUIT/REDIRECT); treating
                // that as Ended made every manual skip publish a bogus `ended`
                // and would double-advance once auto-advance is wired.
                if data.is_null() {
                    tracing::warn!("mpv actor: END_FILE with null payload; ignoring");
                    return None;
                }
                // SAFETY: mpv guarantees a valid `mpv_event_end_file` for
                // END_FILE; we read only the leading `reason` field.
                let reason = unsafe { (*(data as *const ffi::mpv_event_end_file)).reason };
                match reason {
                    ffi::eof_reason::EOF | ffi::eof_reason::ERROR => Some(BackendEvent::Ended),
                    _ => None,
                }
            }
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
                let value_ptr = unsafe { (*prop).data };
                // The payload carries the property's *native* format, which is
                // exactly why each property is observed with its own format: a
                // FLAG requested as DOUBLE yields NONE on every notification.
                // NONE means "unavailable right now" (no file loaded yet) and is
                // dropped rather than guessed.
                let value = match fmt {
                    ffi::format_::DOUBLE => unsafe { ffi::read_double(value_ptr) },
                    ffi::format_::FLAG => unsafe { ffi::read_flag(value_ptr) },
                    _ => None,
                };
                match (name, value) {
                    (Some(name), Some(value)) => {
                        Some(BackendEvent::PropertyChanged { name, value })
                    }
                    _ => None,
                }
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

    fn observe_continuous(&mut self) {
        // This is the call whose absence froze 播放进度条 and the lyrics: with no
        // `mpv_observe_property` subscription the actor never received a single
        // `time-pos`/`duration` change, so the UI snapshot's position/duration
        // stayed `None` even though the file was audibly playing.
        //
        // The same call is also what keeps `volume` / `muted` / transport state
        // honest: those are read back from mpv instead of being the actor's
        // private guess, so a write mpv did not honour can no longer leave the
        // UI claiming 有声 while the output is muted.
        for (name, format) in OBSERVED_PROPERTIES {
            let Ok(cname) = std::ffi::CString::new(*name) else {
                continue;
            };
            // SAFETY: actor thread owns the handle; `cname` outlives the call.
            // The format must be the property's own (see OBSERVED_PROPERTIES):
            // a wrong one is accepted here but silently yields NONE payloads.
            if let Err(error) = unsafe {
                self.handle
                    .observe_property(&self.sys, 0, cname.as_c_str(), *format)
            } {
                tracing::warn!(
                    property = *name,
                    format = *format,
                    error = %error,
                    "mpv actor: observe_property failed; this signal will not reach the UI"
                );
            }
        }
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
            BackendProperty::Pause(on) => {
                let arg = c(if on { "yes" } else { "no" });
                // SAFETY: `set pause yes|no` is a stable mpv command; it is how
                // the audio is actually stopped and resumed.
                unsafe { self.handle.command(&self.sys, &[c("set"), c("pause"), arg]) }.is_ok()
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
#[allow(clippy::struct_excessive_bools)] // Independent capability switches of a scripted backend, not state combinations.
struct TestBackend {
    events: std::collections::VecDeque<BackendEvent>,
    loads: Vec<String>,
    /// Shared recorder so tests can observe the writes the actor issued.
    props: Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    /// How many times the loop asked for the continuous-property subscription.
    /// The real backend's `mpv_observe_property` is what makes the progress bar
    /// and the lyrics move, so a test can prove the `FileLoaded` path asks.
    observations: Arc<std::sync::atomic::AtomicUsize>,
    /// When true, the next `write_property` is rejected (authoritative
    /// rollback test, task 8.8).
    fail_next_property: bool,
    /// When true, *every* `write_property` is rejected — a simulated backend
    /// in a read-only/failed state (task 8.8 rollback over a batch of writes).
    fail_all_properties: bool,
    /// When true, every [`Backend::queue_load`] schedules a `FileLoaded`, the
    /// way the real backend's `loadfile` does.
    ///
    /// A *scripted* event list cannot express "one `FILE_LOADED` per load": the
    /// pump drains it regardless of which load is in flight, so a second load
    /// would consume an event that belonged to the first and then sit at
    /// `Loading` forever. Load-intent tests need the load to *cause* the event.
    file_loaded_on_load: bool,
    /// When set, `observe_continuous` queues the audio properties the way mpv
    /// does on every subscription: mpv replays the *current* value of each
    /// observed property as soon as it is observed. `(volume 0–100, mute,
    /// pause)` — mpv's native units, not ours.
    ///
    /// This is what makes the read-back path testable: the actor used to trust
    /// the writes it just issued, so a value mpv held differently never reached
    /// the snapshot. A scripted list alone cannot express "the subscription
    /// produces events", and without that the whole driver is untestable.
    audio_replay_on_observe: Option<(f64, bool, bool)>,
    terminated: bool,
}

#[cfg(test)]
impl TestBackend {
    fn new(events: Vec<BackendEvent>) -> Self {
        Self {
            events: events.into(),
            loads: Vec::new(),
            props: Arc::new(std::sync::Mutex::new(Vec::new())),
            observations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_next_property: false,
            fail_all_properties: false,
            file_loaded_on_load: false,
            audio_replay_on_observe: None,
            terminated: false,
        }
    }
}

#[cfg(test)]
impl Backend for TestBackend {
    fn pump(&mut self) -> Option<BackendEvent> {
        self.events.pop_front().map_or_else(
            || {
                // Idle: small artificial delay to bound the loop's CPU cost.
                std::thread::sleep(std::time::Duration::from_millis(1));
                None
            },
            Some,
        )
    }

    fn queue_load(&mut self, path: &Path) {
        self.loads.push(path.display().to_string());
        if self.file_loaded_on_load {
            // `loadfile` ⇒ FILE_LOADED, exactly like the real backend.
            self.events.push_back(BackendEvent::FileLoaded);
        }
    }

    fn observe_continuous(&mut self) {
        self.observations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // mpv replays the current value of every observed property the moment
        // it is observed. Modelling that here is what lets a test drive the
        // read-back path — without it the actor's own writes are the only
        // thing any test can ever observe, which is precisely the blind spot
        // that let `muted` diverge from mpv's real `mute`.
        if let Some((volume, mute, pause)) = self.audio_replay_on_observe {
            self.events.push_back(BackendEvent::PropertyChanged {
                name: "volume".into(),
                value: volume,
            });
            self.events.push_back(BackendEvent::PropertyChanged {
                name: "mute".into(),
                value: f64::from(u8::from(mute)),
            });
            self.events.push_back(BackendEvent::PropertyChanged {
                name: "pause".into(),
                value: f64::from(u8::from(pause)),
            });
        }
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
    /// Live snapshot subscribers, each with a bounded mailbox (capacity 1). The
    /// actor loop fans every published snapshot out to them; the composition
    /// root registers the UI forwarder here. Without a *live* sender the
    /// `player://snapshot` stream is silently empty and every snapshot-driven
    /// surface (player bar, queue, progress) freezes at its initial value.
    subscribers: Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>>,
    stopped: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
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
                let mut loop_state =
                    ActorLoop::new(backend, snapshot_join.clone(), subscribers_join);
                loop_state.resolver = resolver;
                loop_state.run(rx);
                // Loop exits only on Shutdown / backend shutdown; ordered
                // teardown happens here, on the actor thread, before joining.
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
                    let pause_applied = self
                        .backend
                        .write_property(BackendProperty::Pause(self.intended_paused));
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
                    // …and publish the state the intent implies. `FileLoaded`
                    // alone is not "playing": a primed / restored load is
                    // decoded but silent, so publishing `Playing` there makes
                    // the UI draw a pause icon and let `useSmoothPosition`
                    // interpolate a progress bar forward for a track that is
                    // not audibly playing. A *rejected* pause write cannot tell
                    // which flag mpv ended up with, so the snapshot takes the
                    // silent reading — 绝不报一个听不到的 `Playing`; the
                    // transport button simply retries the write.
                    let silent = self.intended_paused || !pause_applied;
                    self.state = if silent {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    };
                    self.publish(self.generation);
                }
                Some(BackendEvent::Ended) => {
                    self.state = PlaybackState::Ended;
                    self.position = None;
                    self.publish(self.generation);
                }
                Some(BackendEvent::PropertyChanged { name, value }) => {
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
                self.generation += 1;
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
                self.generation += 1;
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
                self.load_path(&path);
            }
            PlayerCommand::Play => {
                // The write is the point: previously this only flipped the
                // actor's own state flag, so 播放/暂停 changed the icon while the
                // audio kept running.
                self.intended_paused = false;
                if self.backend.write_property(BackendProperty::Pause(false))
                    && self.state.can_transition_to(PlaybackState::Playing)
                {
                    self.state = PlaybackState::Playing;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::Pause => {
                self.intended_paused = true;
                if self.backend.write_property(BackendProperty::Pause(true))
                    && self.state.can_transition_to(PlaybackState::Paused)
                {
                    self.state = PlaybackState::Paused;
                    self.publish(self.generation);
                }
            }
            PlayerCommand::TogglePlayPause => {
                let target = match self.state {
                    PlaybackState::Playing => Some(PlaybackState::Paused),
                    PlaybackState::Paused => Some(PlaybackState::Playing),
                    _ => None,
                };
                if let Some(target) = target {
                    let pause = target == PlaybackState::Paused;
                    if self.backend.write_property(BackendProperty::Pause(pause)) {
                        // The user's latest transport choice is the intent a
                        // pending load must not override when it lands.
                        self.intended_paused = pause;
                        self.state = target;
                        self.publish(self.generation);
                    }
                }
            }
            PlayerCommand::Next | PlayerCommand::Previous => {
                // The coordinator advances the queue and issues a new load; mpv
                // reaches natural EOF and reports Ended. We surface Ended so
                // the coordinator knows to advance.
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

    fn load_path(&mut self, path: &Path) {
        self.generation += 1;
        self.pending_seek = None;
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

    /// An empty subscriber registry for tests that drive `ActorLoop` directly
    /// (no forwarder attached).
    fn subscribers_stub() -> Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>> {
        Arc::new(Mutex::new(Vec::new()))
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
            None,
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    /// Spawn an actor whose `TestBackend` counts how often the loop asked it to
    /// subscribe to the continuous properties — the request the real backend
    /// turns into `mpv_observe_property`. Without it the actor receives no
    /// `time-pos`/`duration` events and every live surface freezes.
    fn spawn_test_observed(
        events: Vec<BackendEvent>,
    ) -> (
        PlayerActor,
        Arc<RwLock<PlayerSnapshot>>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let snapshot = snapshot_stub();
        let observations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observations_join = observations.clone();
        let actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> {
                let mut b = TestBackend::new(events);
                b.observations = observations_join;
                Ok(b)
            },
            snapshot.clone(),
            None,
        )
        .expect("spawn");
        (actor, snapshot, observations)
    }

    /// A backend whose `queue_load` emits its `FileLoaded` event afterwards,
    /// matching the causal ordering of libmpv instead of relying on a startup
    /// race in a scripted event queue.
    type FileLoadedActorFixture = (
        PlayerActor,
        Arc<RwLock<PlayerSnapshot>>,
        Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    );

    fn spawn_test_file_loaded_on_load() -> FileLoadedActorFixture {
        let snapshot = snapshot_stub();
        let props = Arc::new(std::sync::Mutex::new(Vec::new()));
        let props_join = props.clone();
        let actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> {
                let mut backend = TestBackend::new(vec![]);
                backend.props = props_join;
                backend.file_loaded_on_load = true;
                Ok(backend)
            },
            snapshot.clone(),
            None,
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    /// A `LoadTemporary` command for the throwaway test path.
    fn load_temporary(path: &str) -> PlayerCommand {
        PlayerCommand::LoadTemporary {
            display_name: "a.flac".into(),
            path: std::path::PathBuf::from(path),
            session_id: echo_core::domain::ids::PlaybackSessionId::new(),
        }
    }

    /// Wait (bounded, ~1s) until `cond` holds on the shared snapshot. The bound
    /// must clear a loaded CI runner as well as a cold local machine: when the
    /// whole workspace test suite runs in parallel, the actor thread competes
    /// with hundreds of others and a 500 ms budget was occasionally not enough.
    fn wait_for(
        snapshot: &Arc<RwLock<PlayerSnapshot>>,
        mut cond: impl FnMut(&PlayerSnapshot) -> bool,
    ) -> PlayerSnapshot {
        for _ in 0..200 {
            let s = snapshot.read().expect("snapshot poisoned").clone();
            if cond(&s) {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        snapshot.read().expect("snapshot poisoned").clone()
    }

    fn wait_for_property_writes(
        props: &Arc<std::sync::Mutex<Vec<BackendProperty>>>,
        count: usize,
    ) -> Vec<BackendProperty> {
        // Same ~1s budget as `wait_for`: the property writes land from the actor
        // thread, which is starved when the whole workspace test suite runs in
        // parallel on a loaded CI runner.
        for _ in 0..200 {
            let writes = props.lock().expect("properties poisoned").clone();
            if writes.len() >= count {
                return writes;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        props.lock().expect("properties poisoned").clone()
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
        // `TestBackend::load` emits FileLoaded *after* the actor receives this
        // command, just like mpv. Pre-seeding the event races actor startup and
        // can consume it while stopped, which tests neither the load transition
        // nor the production event ordering.
        let (mut actor, snapshot, _props) = spawn_test_file_loaded_on_load();
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
    // 音量的静音分叉 / 音频属性回读（用户报的"调大音量反而没声"）
    // ------------------------------------------------------------------

    /// Spawn an actor whose backend does the two things the real mpv backend
    /// does and a scripted event list cannot express: it emits a `FileLoaded`
    /// per load, and it **replays** mpv's current audio properties on every
    /// subscription (mpv does exactly that the moment a property is observed).
    #[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
    fn spawn_test_audio_readback(
        replay: (f64, bool, bool),
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
                let mut b = TestBackend::new(vec![]);
                b.props = props_join;
                b.file_loaded_on_load = true;
                b.audio_replay_on_observe = Some(replay);
                Ok(b)
            },
            snapshot.clone(),
            None,
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    #[test]
    fn the_audio_controls_are_read_back_from_mpv_on_every_load() {
        // The snapshot must report what mpv *holds*, not what the actor last
        // commanded. With mpv replaying 音量 40 / 静音 / 暂停 on subscription,
        // the snapshot has to say exactly that — even though this very load
        // asked for playback. Before the audio controls were observed, the
        // actor's own opinion was final: UI 说未静音、实际静音（下一次「调大音量」
        // 正好撞上），UI 说在播、时钟却不走。
        let (mut actor, snapshot, props) = spawn_test_audio_readback((40.0, true, true));
        actor.send(load_temporary("/music/a.flac")).unwrap();

        let settled = wait_for(&snapshot, |s| s.state == PlaybackState::Paused && s.muted);
        assert_eq!(
            settled.state,
            PlaybackState::Paused,
            "mpv says still paused"
        );
        assert!(settled.muted, "mpv says muted");
        assert!(
            (settled.volume - 0.4).abs() < 1e-9,
            "mpv's 0–100 volume lands in the snapshot as 0.0–1.0 [got {:?}]",
            settled.volume
        );
        // The intent was still expressed to mpv — the divergence is resolved by
        // mpv's answer, not by the actor ignoring the load's intent.
        assert!(
            props
                .lock()
                .unwrap()
                .contains(&BackendProperty::Pause(false)),
            "a play-intent load still clears mpv's pause flag"
        );
        actor.shutdown();
    }

    #[test]
    fn pause_only_rewrites_the_transport_states_it_can_express() {
        use PlaybackState as S;
        // The flag means Playing ⇄ Paused …
        assert_eq!(
            ActorLoop::<TestBackend>::transport_from_pause(S::Playing, true),
            Some(S::Paused)
        );
        assert_eq!(
            ActorLoop::<TestBackend>::transport_from_pause(S::Paused, false),
            Some(S::Playing)
        );
        // … a no-op when it already agrees …
        assert_eq!(
            ActorLoop::<TestBackend>::transport_from_pause(S::Playing, false),
            None
        );
        // … and nothing else. A late `pause=yes` echo must not turn an
        // `Ended`/`Stopped`/`Failed` snapshot into Paused — that would show a
        // finished track as loaded playback again.
        for state in [S::Ended, S::Stopped, S::Failed, S::Loading] {
            assert_eq!(
                ActorLoop::<TestBackend>::transport_from_pause(state, true),
                None,
                "pause must not rewrite {state:?}"
            );
            assert_eq!(
                ActorLoop::<TestBackend>::transport_from_pause(state, false),
                None,
                "pause must not rewrite {state:?}"
            );
        }
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
            None,
        )
        .expect("spawn");
        (actor, snapshot, props)
    }

    /// Spawn an actor over a backend that resolves library songs through
    /// `resolver` and emits a `FileLoaded` for **every** queued load (see
    /// [`TestBackend::file_loaded_on_load`]), so a test can drive a real
    /// `load → FileLoaded → load` sequence. `fail_all` makes every property
    /// write fail, for the "never publish a state mpv did not reach" assertions.
    #[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
    fn spawn_test_load_driven(
        resolver: SongResolver,
        fail_all: bool,
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
                let mut b = TestBackend::new(vec![]);
                b.props = props_join;
                b.file_loaded_on_load = true;
                b.fail_all_properties = fail_all;
                Ok(b)
            },
            snapshot.clone(),
            Some(resolver),
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
        // The un-mute has to reach mpv, not just this struct: `volume` and
        // `mute` are independent properties there, so a lone `set volume`
        // would leave `mute=yes` in force while the snapshot claimed 未静音 —
        // UI says sound, speaker has none.
        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                BackendProperty::Mute(true),
                BackendProperty::Volume(0.4),
                BackendProperty::Mute(false),
            ],
            "调音量必须把 mpv 的 mute 一起落下，否则 UI 说有声、扬声器是静音"
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
    fn actor_set_mute_is_absolute_and_idempotent() {
        // Session restore re-applies the persisted output state; replaying it
        // must land on the same value instead of flipping. `ToggleMute` is
        // relative, so a second replay would have un-muted the player.
        let (mut actor, snapshot, props) = spawn_test(vec![]);
        actor.send(PlayerCommand::SetVolume(0.6)).unwrap();
        actor.send(PlayerCommand::SetMute(true)).unwrap();
        let muted = wait_for(&snapshot, |s| s.muted);
        assert!(
            (muted.volume - 0.6).abs() < 1e-9,
            "muting keeps the volume field (mute is only the flag, like mpv)"
        );
        // The same absolute value twice: no second backend write, no flip.
        actor.send(PlayerCommand::SetMute(true)).unwrap();
        actor.send(PlayerCommand::SetMute(true)).unwrap();
        // A later command proves the actor drained the no-ops in order.
        actor.send(PlayerCommand::Seek(1.0)).unwrap();
        let _ = wait_for(&snapshot, |s| s.position == Some(1.0));
        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                BackendProperty::Volume(0.6),
                BackendProperty::Mute(true),
                BackendProperty::Seek(1.0),
            ],
            "re-applying an already-satisfied absolute mute must be a no-op"
        );
        assert!(snapshot.read().unwrap().muted);
        actor.shutdown();
    }

    #[test]
    fn actor_set_mute_false_restores_remembered_volume() {
        let (mut actor, snapshot, props) = spawn_test(vec![]);
        actor.send(PlayerCommand::SetVolume(0.35)).unwrap();
        actor.send(PlayerCommand::SetMute(true)).unwrap();
        let muted = wait_for(&snapshot, |s| s.muted);
        assert!((muted.volume - 0.35).abs() < 1e-9);
        // Restoring the un-muted state brings the audible volume back.
        actor.send(PlayerCommand::SetMute(false)).unwrap();
        let unmuted = wait_for(&snapshot, |s| !s.muted && (s.volume - 0.35).abs() < 1e-9);
        assert!((unmuted.volume - 0.35).abs() < 1e-9);
        // Already un-muted → no further write.
        actor.send(PlayerCommand::SetMute(false)).unwrap();
        actor.send(PlayerCommand::Seek(2.0)).unwrap();
        let _ = wait_for(&snapshot, |s| s.position == Some(2.0));
        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                BackendProperty::Volume(0.35),
                BackendProperty::Mute(true),
                BackendProperty::Mute(false),
                BackendProperty::Seek(2.0),
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

    // ------------------------------------------------------------------
    // 8.4: snapshot subscription (the `player://snapshot` stream's source)
    // ------------------------------------------------------------------

    #[test]
    fn subscribe_snapshots_delivers_published_snapshots() {
        // Regression: `subscribe_snapshots` used to hand back a receiver whose
        // sender was dropped on the spot, so the forwarder thread exited
        // immediately and the UI never saw a single snapshot. Playback worked
        // (the actor loaded and played the file) while the player bar stayed
        // blank — the failure mode is invisible to any test that only reads the
        // shared snapshot, so this asserts the *subscription* path itself.
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        let rx = actor.subscribe_snapshots();

        actor.send(PlayerCommand::SetVolume(0.5)).unwrap();

        let snap = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("a subscriber must receive the snapshot published after a command");
        assert!((snap.volume - 0.5).abs() < f64::EPSILON);
        actor.shutdown();
    }

    #[test]
    fn every_subscriber_receives_the_same_snapshot() {
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        let rx_a = actor.subscribe_snapshots();
        let rx_b = actor.subscribe_snapshots();
        let timeout = std::time::Duration::from_secs(2);

        actor.send(PlayerCommand::SetVolume(0.25)).unwrap();

        let a = rx_a.recv_timeout(timeout).expect("subscriber A");
        let b = rx_b.recv_timeout(timeout).expect("subscriber B");
        assert_eq!(
            a, b,
            "fan-out delivers the same snapshot to every subscriber"
        );
        actor.shutdown();
    }

    #[test]
    fn dropped_subscriber_is_pruned_and_live_ones_keep_receiving() {
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        let dead = actor.subscribe_snapshots();
        drop(dead); // the forwarder thread ended / the UI went away

        let live = actor.subscribe_snapshots();
        actor.send(PlayerCommand::SetVolume(0.75)).unwrap();

        let snap = live
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("a live subscriber must still receive after a peer is dropped");
        assert!((snap.volume - 0.75).abs() < f64::EPSILON);
        actor.shutdown();
    }

    #[test]
    fn slow_subscriber_skips_snapshots_instead_of_stalling_the_actor() {
        // The mailbox is capacity-1: a consumer that never drains must not block
        // the actor loop. Three publishes while nobody reads leave the *oldest*
        // snapshot queued and drop the newer ones (each snapshot is full state,
        // so the consumer is never shown stale-then-inconsistent data).
        let (mut actor, _snapshot, _props) = spawn_test(vec![]);
        let rx = actor.subscribe_snapshots();

        actor.send(PlayerCommand::SetVolume(0.2)).unwrap();
        actor.send(PlayerCommand::SetVolume(0.4)).unwrap();
        actor.send(PlayerCommand::SetVolume(0.6)).unwrap();

        let snap = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the not-yet-drained mailbox still delivers");
        assert!(
            (snap.volume - 0.2).abs() < f64::EPSILON,
            "the actor dropped later snapshots rather than blocking on the slow consumer"
        );
        actor.shutdown();
    }

    #[test]
    fn interval_lengths_match_10hz_foreground_and_1hz_background() {
        // The throttle must be 10 Hz foreground (100 ms) and 1 Hz background
        // (1000 ms). Assert the ActorLoop's interval selection directly.
        let snapshot = snapshot_stub();
        let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot, subscribers_stub());
        loop_state.foreground = true;
        assert_eq!(
            loop_state.property_interval(),
            std::time::Duration::from_millis(100)
        );
        loop_state.foreground = false;
        assert_eq!(
            loop_state.property_interval(),
            std::time::Duration::from_secs(1)
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
        let mut loop_state = ActorLoop::new(TestBackend::new(vec![]), snapshot, subscribers_stub());
        loop_state.foreground = true;
        loop_state.position = Some(1.0);
        // Force "now": a publish right after is within the 100 ms interval →
        // throttled away.
        loop_state.last_property_publish = std::time::Instant::now();

        // First publish (interval since `now` ~0 < 100 ms) is throttled away.
        loop_state.publish_throttled(0);
        assert_eq!(loop_state.snapshot.read().unwrap().position, None);

        // Backdate the last publish so the interval has elapsed → publishes.
        loop_state.last_property_publish = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(200))
            .unwrap();
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
    fn transport_pause_and_play_write_the_mpv_pause_flag() {
        // Regression: 播放/暂停 only flipped the actor's own state flag — `pause`
        // was never written to mpv, so the control bar changed its icon while
        // the audio kept playing. Both commands must reach the backend.
        let (mut actor, snapshot, props) = spawn_test(vec![BackendEvent::FileLoaded]);
        actor.send(load_temporary("/music/a.flac")).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

        actor.send(PlayerCommand::Pause).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
        actor.send(PlayerCommand::Play).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![
                // The `LoadTemporary` that got us here is a *play* intent, so
                // FileLoaded clears mpv's pause flag for the new file first.
                BackendProperty::Pause(false),
                BackendProperty::Pause(true),
                BackendProperty::Pause(false),
            ],
            "pause/resume must write mpv's `pause` property, not just the state flag"
        );
        actor.shutdown();
    }

    #[test]
    fn toggle_play_pause_writes_the_pause_flag() {
        let (mut actor, snapshot, props) = spawn_test(vec![BackendEvent::FileLoaded]);
        actor.send(load_temporary("/music/a.flac")).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

        actor.send(PlayerCommand::TogglePlayPause).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
        // The space-bar hotkey path must reach mpv too (the leading write is the
        // load's play intent, re-asserted on FileLoaded).
        assert_eq!(
            wait_for_property_writes(&props, 2),
            vec![BackendProperty::Pause(false), BackendProperty::Pause(true)]
        );
        actor.shutdown();
    }

    #[test]
    fn a_paused_load_publishes_paused_and_reasserts_the_pause_flag() {
        // 冷启动 prime / 会话恢复: the load is decoded but must never sound, and
        // the snapshot must say `Paused` — publishing `Playing` there made the UI
        // draw a pause icon and interpolate the progress bar forward for a silent
        // track (the reported "启动就是正在播放/点歌进度走了却没声音").
        let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
        let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

        actor
            .send(PlayerCommand::LoadLibrarySongPaused {
                song_id: echo_core::domain::ids::SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");

        let primed = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
        assert_eq!(
            primed.state,
            PlaybackState::Paused,
            "a primed load is decoded, not playing"
        );
        // The flag is written before the loadfile AND re-asserted on FileLoaded:
        // mpv keeps a runtime `pause` across `loadfile`, so the intent has to be
        // restated against the file that actually loaded.
        assert_eq!(
            props.lock().unwrap().clone(),
            vec![BackendProperty::Pause(true), BackendProperty::Pause(true)],
            "the paused load must hold mpv's `pause` flag on"
        );
        actor.shutdown();
    }

    #[test]
    fn seek_queued_during_paused_load_runs_after_file_loaded() {
        let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
        let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

        actor
            .send(PlayerCommand::LoadLibrarySongPaused {
                song_id: echo_core::domain::ids::SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send paused load");
        actor
            .send(PlayerCommand::Seek(37.5))
            .expect("queue restore seek");

        let restored = wait_for(&snapshot, |s| {
            s.state == PlaybackState::Paused && s.position == Some(37.5)
        });
        assert_eq!(restored.position, Some(37.5));
        assert_eq!(
            props.lock().expect("property writes").clone(),
            vec![
                BackendProperty::Pause(true),
                BackendProperty::Pause(true),
                BackendProperty::Seek(37.5),
            ],
            "the restore seek must follow FileLoaded's pause write"
        );
        actor.shutdown();
    }

    #[test]
    fn a_rejected_pause_write_on_a_play_load_publishes_paused_not_a_silent_playing() {
        // If the backend rejects the `pause=no` a play-intent load needs, the
        // actor cannot know which flag mpv ended up with. Reporting `Playing`
        // there is exactly the reported bug (icon + moving progress bar, no
        // sound), so the snapshot takes the silent reading instead.
        let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
        let (mut actor, snapshot, _props) = spawn_test_load_driven(resolver, true);
        actor
            .send(PlayerCommand::LoadLibrarySong {
                song_id: echo_core::domain::ids::SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .unwrap();

        let settled = wait_for(&snapshot, |s| s.state == PlaybackState::Paused);
        assert_eq!(
            settled.state,
            PlaybackState::Paused,
            "a rejected un-pause must never be published as Playing"
        );
        actor.shutdown();
    }

    #[test]
    fn a_play_load_clears_the_pause_flag_a_primed_load_left_behind() {
        // The reported bug, in the actor: 冷启动 prime writes `pause=yes`, mpv
        // keeps it across `loadfile` (manual: "if any option is changed at
        // runtime … they are not reset when a new file is played"), so 点歌 by
        // `LoadLibrarySong` used to load a *silent* track while the snapshot said
        // `Playing` — 进度条在走，扬声器没声，只有播放控制栏的 播放 才真的出声。
        let resolver: SongResolver = Arc::new(|_| Ok(std::path::PathBuf::from("/music/a.flac")));
        let (mut actor, snapshot, props) = spawn_test_load_driven(resolver, false);

        actor
            .send(PlayerCommand::LoadLibrarySongPaused {
                song_id: echo_core::domain::ids::SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");
        wait_for(&snapshot, |s| s.state == PlaybackState::Paused);

        actor
            .send(PlayerCommand::LoadLibrarySong {
                song_id: echo_core::domain::ids::SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");
        let playing = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
        assert_eq!(playing.state, PlaybackState::Playing);

        let writes = props.lock().unwrap().clone();
        assert_eq!(
            writes.last(),
            Some(&BackendProperty::Pause(false)),
            "a play-intent load must clear mpv's `pause` flag, or the track stays \
             silent while the UI shows it playing [writes={writes:?}]"
        );
        actor.shutdown();
    }

    #[test]
    fn file_loaded_subscribes_to_the_continuous_properties() {
        // Regression: `mpv_observe_property` was never called, so the actor
        // received no `PROPERTY_CHANGE` at all — `position`/`duration` stayed
        // `None`, freezing 播放进度条 and the synced-lyrics highlight while the
        // song played. The load path is where the subscription must happen.
        let (mut actor, snapshot, observations) =
            spawn_test_observed(vec![BackendEvent::FileLoaded]);
        assert_eq!(
            observations.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing is subscribed before a file is loaded"
        );

        actor.send(load_temporary("/music/a.flac")).unwrap();
        wait_for(&snapshot, |s| s.state == PlaybackState::Playing);

        assert!(
            observations.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "a loaded file must subscribe to time-pos/duration or the UI has no live values"
        );
        actor.shutdown();
    }

    #[test]
    fn observed_properties_cover_position_duration_and_the_audio_controls() {
        // The subscription set has to include everything the snapshot claims to
        // report: the two the UI reads (progress bar / lyrics) *and* the audio
        // controls, which are what make `volume` / `muted` mpv's values rather
        // than the actor's private guess.
        let names: Vec<&str> = OBSERVED_PROPERTIES.iter().map(|(name, _)| *name).collect();
        for required in ["time-pos", "duration", "volume", "mute", "pause"] {
            assert!(
                names.contains(&required),
                "{required} must be observed or the snapshot reports a guess \
                 instead of the value mpv actually holds [set={names:?}]"
            );
        }
        // …and each with the format that actually delivers a payload. Measured
        // against the vendored libmpv: observing `mute`/`pause` as DOUBLE is
        // accepted by mpv but every notification then carries MPV_FORMAT_NONE
        // (no value) — a subscription that looks fine and delivers nothing.
        let format_of = |want: &str| {
            OBSERVED_PROPERTIES
                .iter()
                .find(|(name, _)| *name == want)
                .map(|(_, format)| *format)
        };
        assert_eq!(format_of("time-pos"), Some(ffi::format_::DOUBLE));
        assert_eq!(format_of("duration"), Some(ffi::format_::DOUBLE));
        assert_eq!(format_of("volume"), Some(ffi::format_::DOUBLE));
        assert_eq!(format_of("mute"), Some(ffi::format_::FLAG));
        assert_eq!(format_of("pause"), Some(ffi::format_::FLAG));
    }

    #[test]
    fn a_flag_property_event_is_decoded_from_its_native_format() {
        // The real backend decodes a FLAG payload (a C `int`) into the same
        // `f64` event vocabulary. If that path regressed, `mute`/`pause`
        // notifications would be dropped as "unavailable" forever and the
        // snapshot would silently fall back to the actor's guesses.
        assert_eq!(ffi::format_::FLAG, 3);
        let on = [1i32];
        // SAFETY: `on` is a live `int` for the duration of the call.
        let decoded = unsafe { ffi::read_flag(on.as_ptr().cast()) };
        assert_eq!(decoded, Some(1.0), "FLAG 1 → 1.0");
        let off = [0i32];
        // SAFETY: as above.
        assert_eq!(unsafe { ffi::read_flag(off.as_ptr().cast()) }, Some(0.0));
        // SAFETY: a null data pointer is explicitly allowed (unavailable).
        assert_eq!(unsafe { ffi::read_flag(std::ptr::null()) }, None);
    }

    /// The vendored macOS libmpv, if this host has one — the same skip rule the
    /// `player_smoke` suite uses, so a host without the bundle still passes.
    fn vendored_libmpv() -> Option<std::path::PathBuf> {
        // The vendored library is the macOS dylib bundled into the .app; its
        // Mach-O image cannot be dlopened on Windows/Linux (the workspace
        // builds these tests everywhere, and `--all-features` gates run them
        // on every platform). Resolve it only on macOS and let the caller's
        // SKIP branch handle the rest.
        if !cfg!(target_os = "macos") {
            return None;
        }
        for candidate in [
            "../../apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
            "apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
        ] {
            let path = std::path::Path::new(candidate);
            if path.exists() {
                return Some(path.to_path_buf());
            }
        }
        None
    }

    #[test]
    fn libmpv_treats_volume_and_mute_as_independent_properties() {
        // The causal chain behind 调大音量反而没声: in mpv, `set volume` does
        // **not** clear `mute`. The actor used to write only the volume and then
        // record `muted: false` in its own snapshot, so the UI claimed sound
        // while the output was silenced — and the next 调大音量 landed on it.
        // Checked against mpv, because "surely setting the volume unmutes" is
        // precisely the assumption that produced the bug.
        let Some(lib) = vendored_libmpv() else {
            eprintln!("SKIP: no vendored libmpv on this host");
            return;
        };
        // SAFETY: this test thread owns the handle for the whole test.
        unsafe {
            let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
            let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");
            let muted = std::ffi::CString::new("mute").expect("no NUL");
            handle
                .observe_property(&sys, 0, muted.as_c_str(), ffi::format_::FLAG)
                .expect("observe mute");

            // Wait (bounded) for mpv to report `mute == want`; `false` on timeout.
            let wait_mute = |want: f64| -> bool {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while std::time::Instant::now() < deadline {
                    let ev = handle.wait_event(&sys, 0.05);
                    if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                        continue;
                    }
                    let data = (*ev).data;
                    if data.is_null() {
                        continue;
                    }
                    let property = data as *const ffi::mpv_event_property;
                    if ffi::read_c_str((*property).name).as_deref() != Some("mute")
                        || (*property).format != ffi::format_::FLAG
                    {
                        continue;
                    }
                    if ffi::read_flag((*property).data) == Some(want) {
                        return true;
                    }
                }
                false
            };
            let set = |name: &str, value: &str| {
                let args: Vec<std::ffi::CString> = ["set", name, value]
                    .iter()
                    .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                    .collect();
                handle
                    .command(&sys, &args)
                    .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
            };

            set("mute", "yes");
            assert!(wait_mute(1.0), "mute=yes must take effect");
            // The load path's volume write must not be assumed to un-mute.
            set("volume", "40");
            assert!(
                !wait_mute(0.0),
                "`set volume` must NOT clear mpv's `mute` — that is why the actor has to \
                 write both, and why only writing the volume made the UI claim 未静音"
            );
            set("mute", "no");
            assert!(wait_mute(0.0), "the explicit un-mute must take effect");
            handle.terminate(&sys);
        }
    }

    #[test]
    fn observed_properties_really_deliver_values_from_libmpv() {
        // The subscription table is a claim about *mpv's* behaviour, so it is
        // checked against mpv rather than trusted: observing `mute`/`pause` with
        // DOUBLE is **accepted** by `mpv_observe_property` and then delivers
        // MPV_FORMAT_NONE forever — a subscription that looks healthy and
        // reports nothing. That is how the audio controls ended up "observed"
        // nowhere at all, leaving the snapshot to report the actor's own guess
        // (UI 说未静音、实际静音；UI 说在播、时钟不走).
        //
        // Skipped only when the host has no vendored libmpv.
        let Some(lib) = vendored_libmpv() else {
            eprintln!("SKIP: no vendored libmpv on this host");
            return;
        };
        // SAFETY: this test thread owns the handle for the whole test and
        // terminates it before returning.
        unsafe {
            let sys = ffi::MpvSys::load(&lib).expect("dlopen the vendored libmpv");
            let mut handle = ffi::Handle::create(&sys, &[], &[]).expect("create mpv handle");

            for (name, format) in OBSERVED_PROPERTIES {
                let cname = std::ffi::CString::new(*name).expect("no NUL");
                handle
                    .observe_property(&sys, 0, cname.as_c_str(), *format)
                    .unwrap_or_else(|e| panic!("observe {name} as {format}: {e}"));
            }
            // Drive one change per property, so each observation must deliver.
            for (name, value) in [("pause", "yes"), ("mute", "yes"), ("volume", "40")] {
                let args: Vec<std::ffi::CString> = ["set", name, value]
                    .iter()
                    .map(|s| std::ffi::CString::new(*s).expect("no NUL"))
                    .collect();
                handle
                    .command(&sys, &args)
                    .unwrap_or_else(|e| panic!("set {name} {value}: {e}"));
            }

            let mut seen: Vec<(String, f64)> = Vec::new();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                let ev = handle.wait_event(&sys, 0.05);
                if ev.is_null() || (*ev).event_id != ffi::event_id::PROPERTY_CHANGE {
                    continue;
                }
                let data = (*ev).data;
                if data.is_null() {
                    continue;
                }
                let property = data as *const ffi::mpv_event_property;
                let Some(name) = ffi::read_c_str((*property).name) else {
                    continue;
                };
                let payload = match (*property).format {
                    ffi::format_::DOUBLE => ffi::read_double((*property).data),
                    ffi::format_::FLAG => ffi::read_flag((*property).data),
                    _ => None,
                };
                if let Some(value) = payload {
                    seen.push((name, value));
                }
            }
            handle.terminate(&sys);

            for wanted in ["pause", "mute", "volume"] {
                assert!(
                    seen.iter().any(|(name, _)| name == wanted),
                    "mpv delivered no value for `{wanted}` with the format the actor observes \
                     it with — the snapshot would silently fall back to a guess [seen={seen:?}]"
                );
            }
            // `pause`/`mute` are observed as MPV_FORMAT_FLAG and `volume` as a
            // DOUBLE, but all three arrive through one payload, so every value
            // is compared with the same tolerance `volume` already uses.
            assert!(
                seen.iter()
                    .any(|(name, v)| name == "pause" && (*v - 1.0).abs() < 1e-9),
                "pause=yes must arrive as 1.0 [seen={seen:?}]"
            );
            assert!(
                seen.iter()
                    .any(|(name, v)| name == "mute" && (*v - 1.0).abs() < 1e-9),
                "mute=yes must arrive as 1.0 [seen={seen:?}]"
            );
            assert!(
                seen.iter()
                    .any(|(name, v)| name == "volume" && (*v - 40.0).abs() < 1e-9),
                "set volume 40 must arrive as 40.0 [seen={seen:?}]"
            );
        }
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
            None,
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
        // The combined set must (a) disable user config/scripts/ytdl,
        // (b) forbid everything but the local file protocol, and (c) force pure
        // audio. Hardening intent is declared across the required + optional
        // groups; the split only reflects what a build can accept.
        let names: Vec<&str> = HARDENED_REQUIRED
            .iter()
            .chain(HARDENED_OPTIONAL)
            .map(|(n, _)| *n)
            .collect();
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
        let whitelist = HARDENED_OPTIONAL
            .iter()
            .find(|(n, _)| *n == "protocol-whitelist")
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(whitelist, "file");
        // The strictly-required set must stay minimal and genuinely loadable on
        // every libmpv build, while config/video/vo are true hard boundaries.
        assert!(
            HARDENED_REQUIRED.iter().all(|(n, _)| *n == "config"
                || *n == "video"
                || *n == "vo"
                || *n == "audio-display"),
            "required options are exactly the audio-only hard boundaries"
        );
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

    #[test]
    fn load_library_song_resolves_and_loads_path() {
        // With a resolver supplied, `LoadLibrarySong` resolves SongId → path on
        // the actor thread and queues a real load; a following FileLoaded drives
        // Loading → Playing (task 10.6 / 8.3 playback path).
        use echo_core::domain::ids::SongId;

        // Use a temp file path so we can assert the exact resolved path reached
        // the backend without leaking anything to DTOs.
        let dir = tempfile::tempdir().expect("tempdir");
        let track_path = dir.path().join("track.flac");
        let expected = track_path;
        let song_id = SongId::new();
        let resolver: SongResolver = Arc::new(move |id| {
            assert_eq!(id, song_id, "resolver given a different song");
            Ok(expected.clone())
        });

        let snapshot = snapshot_stub();
        // A FileLoaded event proves the load reached the backend and succeeded.
        let mut actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> {
                Ok(TestBackend::new(vec![BackendEvent::FileLoaded]))
            },
            snapshot.clone(),
            Some(resolver),
        )
        .expect("spawn");
        actor
            .send(PlayerCommand::LoadLibrarySong {
                song_id,
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");

        let s = wait_for(&snapshot, |s| s.state == PlaybackState::Playing);
        assert_eq!(s.state, PlaybackState::Playing, "load should reach Playing");
        actor.shutdown();
    }

    #[test]
    fn load_library_song_without_resolver_goes_failed() {
        // No resolver (or a resolution error) must surface `Failed`, never a
        // broken `Playing`; the coordinator's error-skip advances past it.
        use echo_core::domain::ids::SongId;

        let snapshot = snapshot_stub();
        let mut actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
            snapshot.clone(),
            None, // no resolver
        )
        .expect("spawn");
        actor
            .send(PlayerCommand::LoadLibrarySong {
                song_id: SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");

        let s = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
        assert_eq!(
            s.state,
            PlaybackState::Failed,
            "unresolvable song -> Failed"
        );
        actor.shutdown();
    }

    #[test]
    fn load_library_song_resolver_error_goes_failed() {
        // A resolver that returns Err must also surface Failed (not Loading),
        // so an unknown song does not spin.
        use echo_core::domain::ids::SongId;

        let snapshot = snapshot_stub();
        let resolver: SongResolver =
            Arc::new(|_| Err(echo_core::error::Error::unavailable("song", "unknown")));
        let mut actor = PlayerActor::spawn_with(
            move || -> Result<TestBackend, ffi::HandleError> { Ok(TestBackend::new(vec![])) },
            snapshot.clone(),
            Some(resolver),
        )
        .expect("spawn");
        actor
            .send(PlayerCommand::LoadLibrarySong {
                song_id: SongId::new(),
                session_id: echo_core::domain::ids::PlaybackSessionId::new(),
            })
            .expect("send");

        let s = wait_for(&snapshot, |s| s.state == PlaybackState::Failed);
        assert_eq!(s.state, PlaybackState::Failed, "resolve error -> Failed");
        actor.shutdown();
    }
}
