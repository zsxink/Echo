//! A scripted backend for testing the actor loop without libmpv.
//!
//! Compiled only for tests: it exists so the actor loop's generation / teardown
//! logic can be verified without loading libmpv.

#![cfg(test)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::backend::{Backend, BackendEvent, BackendProperty};

/// A scripted backend for testing the actor loop without libmpv. It replays a
/// fixed event sequence and records loads / termination / property writes.
#[cfg(test)]
#[allow(clippy::struct_excessive_bools)] // Independent capability switches of a scripted backend, not state combinations.
pub(super) struct TestBackend {
    pub(super) events: std::collections::VecDeque<BackendEvent>,
    pub(super) loads: Vec<String>,
    /// Shared recorder so tests can observe the writes the actor issued.
    pub(super) props: Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    /// How many times the loop asked for the continuous-property subscription.
    /// The real backend's `mpv_observe_property` is what makes the progress bar
    /// and the lyrics move, so a test can prove the `FileLoaded` path asks.
    pub(super) observations: Arc<std::sync::atomic::AtomicUsize>,
    /// When true, the next `write_property` is rejected (authoritative
    /// rollback test, task 8.8).
    pub(super) fail_next_property: bool,
    pub(super) ready_after_pause_rejection: bool,
    /// When true, *every* `write_property` is rejected — a simulated backend
    /// in a read-only/failed state (task 8.8 rollback over a batch of writes).
    pub(super) fail_all_properties: bool,
    /// When true, every [`Backend::queue_load`] schedules a `FileLoaded`, the
    /// way the real backend's `loadfile` does.
    ///
    /// A *scripted* event list cannot express "one `FILE_LOADED` per load": the
    /// pump drains it regardless of which load is in flight, so a second load
    /// would consume an event that belonged to the first and then sit at
    /// `Loading` forever. Load-intent tests need the load to *cause* the event.
    pub(super) file_loaded_on_load: bool,
    /// When set, `observe_continuous` queues the audio properties the way mpv
    /// does on every subscription: mpv replays the *current* value of each
    /// observed property as soon as it is observed. `(volume 0–100, mute,
    /// pause)` — mpv's native units, not ours.
    ///
    /// This is what makes the read-back path testable: the actor used to trust
    /// the writes it just issued, so a value mpv held differently never reached
    /// the snapshot. A scripted list alone cannot express "the subscription
    /// produces events", and without that the whole driver is untestable.
    pub(super) audio_replay_on_observe: Option<(f64, bool, bool)>,
    /// When set, `observe_continuous` queues a `duration` property event the
    /// way mpv replays an observed property's current value on subscription.
    /// Only `duration` — a static fact reported once — is modelled here: a
    /// `time-pos` replay would be published first and reset the throttle
    /// window, swallowing the `duration` snapshot that follows microseconds
    /// later (the real backend keeps ticking `time-pos`, the fake does not).
    pub(super) media_replay_on_observe: Option<f64>,
    /// A controllable native transition completion for first-sample barriers.
    pub(super) effects_ready: Option<Arc<AtomicBool>>,
    pub(super) terminated: bool,
}

#[cfg(test)]
impl TestBackend {
    pub(super) fn new(events: Vec<BackendEvent>) -> Self {
        Self {
            events: events.into(),
            loads: Vec::new(),
            props: Arc::new(std::sync::Mutex::new(Vec::new())),
            observations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_next_property: false,
            ready_after_pause_rejection: false,
            fail_all_properties: false,
            file_loaded_on_load: false,
            audio_replay_on_observe: None,
            media_replay_on_observe: None,
            effects_ready: None,
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
        if let Some(duration) = self.media_replay_on_observe {
            self.events.push_back(BackendEvent::PropertyChanged {
                name: "duration".into(),
                value: duration,
            });
        }
    }

    fn write_property(&mut self, prop: BackendProperty) -> bool {
        if self.fail_next_property {
            self.fail_next_property = false;
            if self.ready_after_pause_rejection && prop == BackendProperty::Pause(true) {
                self.effects_ready
                    .as_ref()
                    .unwrap()
                    .store(true, Ordering::Release);
            }
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

    fn apply_effects(
        &mut self,
        _: &crate::effects::Payload,
        _: crate::effects::ProcessingEnvironment,
        _: &crate::effects::math::EffectsAnalysis,
    ) -> Result<bool, String> {
        self.effects_ready.as_ref().map_or_else(
            || Err("native effects capability unavailable".to_owned()),
            |ready| Ok(ready.load(Ordering::Acquire)),
        )
    }
}
