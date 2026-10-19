//! Backend vocabulary (events, property writes, `OBSERVED_PROPERTIES`) and the
//! `Backend` contract the actor loop consumes.

use std::path::Path;

use super::ffi;

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
pub(super) const OBSERVED_PROPERTIES: &[(&str, i32)] = &[
    ("time-pos", ffi::format_::DOUBLE),
    ("duration", ffi::format_::DOUBLE),
    ("volume", ffi::format_::DOUBLE),
    ("mute", ffi::format_::FLAG),
    ("pause", ffi::format_::FLAG),
    ("audio-params/samplerate", ffi::format_::DOUBLE),
    ("audio-params/channel-count", ffi::format_::DOUBLE),
    ("audio-out-params/samplerate", ffi::format_::DOUBLE),
    ("audio-out-params/channel-count", ffi::format_::DOUBLE),
];

/// A normalized event the actor loop consumes from any backend. The real mpv
/// backend and the test backend both produce these, so tests verify the loop's
/// generation / teardown logic against the same event vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    FileLoaded,
    AudioReconfigured,
    AudioEffectsFailed,
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
pub(super) trait Backend {
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

    fn apply_effects(
        &mut self,
        _payload: &crate::effects::Payload,
        _environment: crate::effects::ProcessingEnvironment,
        _analysis: &crate::effects::math::EffectsAnalysis,
    ) -> Result<bool, String> {
        Err("native effects capability unavailable".to_owned())
    }

    fn bypass_effects(&mut self) -> Result<bool, String> {
        Ok(true)
    }

    fn effects_reconfigured(&mut self) -> bool {
        true
    }

    /// Terminate and release the backend (ordered teardown on the actor thread).
    fn terminate(&mut self);
}
