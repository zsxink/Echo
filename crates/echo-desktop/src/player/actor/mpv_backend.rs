//! The real backend wrapping libmpv via the isolated FFI module.

use std::path::Path;

use super::super::ffi;
use super::backend::{Backend, BackendEvent, BackendProperty, OBSERVED_PROPERTIES};
use super::harden_options::{extra_options, HARDENED_REQUIRED};
use super::native_effects;

/// The real backend wrapping libmpv via [`ffi::MpvSys`] and [`ffi::Handle`].
pub(super) struct MpvBackend {
    pub(super) sys: ffi::MpvSys,
    pub(super) handle: ffi::Handle,
    /// Path queued by [`Backend::queue_load`], issued as `loadfile` on pump.
    pub(super) pending_load: Option<std::path::PathBuf>,
    pub(super) load_pause_rejected: bool,
    pub(super) effects: native_effects::NativeEffects,
    pub(super) events: std::collections::VecDeque<BackendEvent>,
}

impl MpvBackend {
    /// SAFETY: creates a handle on the current thread; the caller (the actor)
    /// owns confinement for the backend's lifetime.
    pub(super) unsafe fn open(libmpv_path: &Path) -> Result<Self, ffi::HandleError> {
        // SAFETY: we load the trusted, pinned artifact and own all symbols.
        let sys =
            unsafe { ffi::MpvSys::load(libmpv_path) }.map_err(|_| ffi::HandleError::Create)?;
        // SAFETY: we own the only handle, created on this thread, hardened with
        // the audio-only / least-privilege option set (task 8.3). `required`
        // options must hold (config/video/vo); `optional` ones are applied
        // best-effort because the vendor `audio-default` build compiles some
        // capabilities out and reports them unknown.
        let handle = unsafe { ffi::Handle::create(&sys, HARDENED_REQUIRED, &extra_options()) }?;
        // SAFETY: initialized handle belongs to this actor thread; metadata is
        // available before playback and its allocation is released inside FFI.
        let command_syntax = native_effects::AfCommandSyntax::from_argument_count(unsafe {
            handle.af_command_argument_count(&sys)
        });
        // SAFETY: initialized handle belongs to this actor thread. Failure to
        // observe disabled filters must not leave effects apparently usable.
        unsafe { handle.request_audio_errors(&sys) }?;
        Ok(Self {
            sys,
            handle,
            pending_load: None,
            load_pause_rejected: false,
            effects: native_effects::NativeEffects::new(command_syntax),
            events: std::collections::VecDeque::new(),
        })
    }

    /// Actor-thread-only wrapper. No native handle or pointer leaves this module.
    pub(super) fn effects_command(&self, args: &[String]) -> Result<(), String> {
        let operation = args.join(" ");
        let args: Result<Vec<_>, _> = args
            .iter()
            .map(|arg| std::ffi::CString::new(arg.as_str()))
            .collect();
        let args = args.map_err(|_| "invalid native effect argument".to_owned())?;
        // SAFETY: the actor owns this initialized handle and all argument
        // strings remain alive, NUL-terminated and immutable for the call.
        unsafe { self.handle.command(&self.sys, &args) }
            .map_err(|error| format!("native effect command `{operation}` rejected: {error}"))
    }

    /// Issue the queued `loadfile` (if any) and then read one event.
    fn pump(&mut self) -> Option<BackendEvent> {
        if !self.load_pause_rejected {
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
        }
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }
        self.read_event(0.005).1
    }

    fn read_event(&mut self, timeout: f64) -> (bool, Option<BackendEvent>) {
        // Pump one event; short timeout so the command loop is re-checked often.
        // SAFETY: actor thread owns handle; event valid for this iteration.
        let ev = unsafe { self.handle.wait_event(&self.sys, timeout) };
        if ev.is_null() {
            return (false, None);
        }
        // SAFETY: `ev` valid for this iteration only.
        let event_id = unsafe { (*ev).event_id };
        let error = unsafe { (*ev).error };
        let data = unsafe { (*ev).data };
        if event_id == ffi::event_id::NONE {
            return (false, None);
        }
        let decoded = (|| match event_id {
            ffi::event_id::LOG_MESSAGE => {
                if data.is_null() {
                    return None;
                }
                // SAFETY: LOG_MESSAGE's leading C strings are valid until the
                // next wait_event. Copy only for classification, never logging
                // arbitrary native text which may contain local file paths.
                let log = unsafe { &*(data.cast::<ffi::mpv_event_log_message>()) };
                let prefix = unsafe { ffi::read_c_str(log.prefix) }.unwrap_or_default();
                let text = unsafe { ffi::read_c_str(log.text) }.unwrap_or_default();
                if native_audio_filter_error(&prefix, &text)
                    && self
                        .effects
                        .failed("native audio filter failed or was disabled")
                {
                    Some(BackendEvent::AudioEffectsFailed)
                } else {
                    None
                }
            }
            ffi::event_id::SHUTDOWN => Some(BackendEvent::Shutdown),
            ffi::event_id::FILE_LOADED => Some(BackendEvent::FileLoaded),
            // MPV_EVENT_AUDIO_RECONFIG is stable event ID 18 in client.h.
            18 => Some(BackendEvent::AudioReconfigured),
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
        })();
        (true, decoded)
    }

    /// Consume native error logs before confirmation. Preserve unrelated events
    /// for the actor; a saturated queue delays confirmation instead of guessing.
    fn drain_effect_events(&mut self) -> bool {
        for _ in 0..64 {
            let (present, event) = self.read_event(0.0);
            if let Some(event) = event {
                self.events.push_back(event);
            }
            if !present {
                return true;
            }
        }
        false
    }

    pub(super) fn confirm_effects_configuration(&mut self) {
        if !self.drain_effect_events() || !self.effects.needs_confirmation() {
            return;
        }
        // SAFETY: each property read runs on the owning actor thread, and frees
        // its returned node. GET_META is rejected for uninitialized graphs.
        let configured = self
            .effects
            .confirmation_labels()
            .iter()
            .all(|label| unsafe { self.handle.audio_filter_configured(&self.sys, label) });
        if configured && self.drain_effect_events() {
            self.effects.configured();
        }
    }
}

pub(super) fn native_audio_filter_error(prefix: &str, text: &str) -> bool {
    (prefix == "af" && text.contains("Disabling filter echo_"))
        || ((prefix == "af" || prefix.ends_with("lavfi"))
            && (text.contains("failed to configure")
                || text.contains("filter graph failed")
                || text.contains("Audio filter initialized failed")))
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
        if matches!(prop, BackendProperty::Pause(_)) && ok {
            self.load_pause_rejected = false;
        } else if prop == BackendProperty::Pause(true) {
            self.load_pause_rejected = true;
        }
        if !ok {
            tracing::warn!(?prop, "mpv actor: runtime property write rejected");
        }
        ok
    }

    fn apply_effects(
        &mut self,
        payload: &crate::effects::Payload,
        environment: crate::effects::ProcessingEnvironment,
        analysis: &crate::effects::math::EffectsAnalysis,
    ) -> Result<bool, String> {
        self.confirm_effects_configuration();
        let mut effects = std::mem::take(&mut self.effects);
        let result = effects.apply(payload, environment, analysis, |args| {
            self.effects_command(args)
        });
        self.effects = effects;
        result
    }

    fn effects_reconfigured(&mut self) -> bool {
        if self.effects.confirmation_labels().is_empty() {
            // Removal itself emits reconfig. It must not retry a failed target.
            return false;
        }
        self.effects.reconfirm();
        true
    }

    fn bypass_effects(&mut self) -> Result<bool, String> {
        let mut effects = std::mem::take(&mut self.effects);
        let result = effects.bypass(|args| self.effects_command(args));
        self.effects = effects;
        if matches!(result, Ok(true)) {
            // A disabled/new load has no effects barrier. A rejected hold from
            // the previous load must not prevent its queued loadfile forever.
            self.load_pause_rejected = false;
        }
        result
    }

    fn terminate(&mut self) {
        // SAFETY: actor thread owns the handle; this is the single free point.
        unsafe { self.handle.terminate(&self.sys) };
    }
}
