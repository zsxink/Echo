//! Latest-intent mailbox and actor-thread effect scheduling.
//! Backend calls execute while the mailbox lock is held: a superseding request
//! can never be accepted between the revision check and the corresponding write.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::super::port::PlayerError;
use super::{Backend, BackendProperty};
use crate::effects::{AppliedState, ChannelLayout, EffectsRuntime, Payload, ProcessingEnvironment};

#[derive(Clone, Debug)]
struct Target {
    revision: u64,
    enabled: bool,
    payload: Payload,
}

#[derive(Default)]
struct Mailbox {
    target: Option<Target>,
    runtime: EffectsRuntime,
    closed: bool,
}

#[derive(Clone, Default)]
pub(super) struct Shared(Arc<(Mutex<Mailbox>, Condvar)>);

impl Shared {
    pub(super) fn submit(
        &self,
        revision: u64,
        enabled: bool,
        payload: Payload,
    ) -> Result<EffectsRuntime, PlayerError> {
        payload
            .validate()
            .map_err(|error| backend_error(&error.to_string()))?;
        let mut mailbox = self
            .0
             .0
            .lock()
            .map_err(|_| backend_error("effect mailbox poisoned"))?;
        if mailbox.closed {
            return Err(PlayerError::ActorClosed);
        }
        if revision <= mailbox.runtime.revision && mailbox.target.is_some() {
            return Err(backend_error("stale effect revision"));
        }
        mailbox.target = Some(Target {
            revision,
            enabled,
            payload,
        });
        mailbox.runtime.revision = revision;
        mailbox.runtime.applied = AppliedState::Pending;
        mailbox.runtime.reason = None;
        Ok(mailbox.runtime.clone())
    }

    pub(super) fn runtime(&self) -> EffectsRuntime {
        self.0
             .0
            .lock()
            .expect("effect mailbox poisoned")
            .runtime
            .clone()
    }

    pub(super) fn bypass(&self, revision: u64) -> Result<EffectsRuntime, PlayerError> {
        let payload = self
            .0
             .0
            .lock()
            .map_err(|_| backend_error("effect mailbox poisoned"))?
            .target
            .as_ref()
            .map_or_else(Payload::default, |target| target.payload.clone());
        self.submit(revision, false, payload)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut mailbox = self
            .0
             .0
            .lock()
            .map_err(|_| backend_error("effect mailbox poisoned"))?;
        loop {
            if mailbox.closed {
                return Err(PlayerError::ActorClosed);
            }
            if mailbox.runtime.revision != revision {
                return Err(backend_error("bypass superseded"));
            }
            match mailbox.runtime.applied {
                AppliedState::Bypassed => return Ok(mailbox.runtime.clone()),
                AppliedState::Failed | AppliedState::Unavailable => {
                    return Err(backend_error("backend could not confirm bypass"))
                }
                _ => {}
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                return Err(backend_error("effect bypass confirmation timed out"));
            }
            mailbox = self
                .0
                 .1
                .wait_timeout(mailbox, timeout)
                .map_err(|_| backend_error("effect mailbox poisoned"))?
                .0;
        }
    }

    pub(super) fn close(&self) {
        if let Ok(mut mailbox) = self.0 .0.lock() {
            mailbox.closed = true;
        }
        self.0 .1.notify_all();
    }
}

fn backend_error(message: &str) -> PlayerError {
    PlayerError::Backend {
        message: message.to_owned(),
    }
}

pub(super) struct Engine {
    pub(super) shared: Shared,
    epoch: u64,
    environment: Option<ProcessingEnvironment>,
    sample_rate: Option<u32>,
    channels: Option<ChannelLayout>,
    completed: Option<(u64, u64)>,
    analysis: Option<(u64, u64, crate::effects::math::EffectsAnalysis)>,
    safe_to_play: bool,
}

impl Engine {
    pub(super) const fn new(shared: Shared) -> Self {
        Self {
            shared,
            epoch: 0,
            environment: None,
            sample_rate: None,
            channels: None,
            completed: None,
            analysis: None,
            safe_to_play: true,
        }
    }

    pub(super) fn begin_load<B: Backend>(&mut self, backend: &mut B) -> bool {
        self.epoch = self.epoch.saturating_add(1);
        self.environment = None;
        self.sample_rate = None;
        self.channels = None;
        self.completed = None;
        let enabled = {
            let mut mailbox = self.shared.0 .0.lock().expect("effect mailbox poisoned");
            mailbox.runtime.playback_epoch = self.epoch;
            mailbox.runtime.processing_rate = None;
            mailbox.runtime.channel_layout = None;
            let enabled = mailbox.target.as_ref().is_some_and(|target| target.enabled);
            if enabled {
                mailbox.runtime.applied = AppliedState::Pending;
            }
            enabled
        };
        if enabled {
            // Holding pause before load prevents an unconfigured first sample.
            return backend.write_property(BackendProperty::Pause(true));
        }
        true
    }

    pub(super) fn reconfigured<B: Backend>(&mut self, backend: &mut B) -> bool {
        if !backend.effects_reconfigured() {
            return false;
        }
        self.epoch = self.epoch.saturating_add(1);
        self.completed = None;
        let mut mailbox = self.shared.0 .0.lock().expect("effect mailbox poisoned");
        mailbox.runtime.playback_epoch = self.epoch;
        if mailbox.target.as_ref().is_some_and(|target| target.enabled) {
            mailbox.runtime.applied = AppliedState::Pending;
        }
        true
    }

    pub(super) fn configuration_failed(&mut self) {
        // A disabled node can fail after an earlier request completed. Revisit
        // its backend receipt so cached Applied cannot conceal that failure.
        let mut mailbox = self.shared.0 .0.lock().expect("effect mailbox poisoned");
        if mailbox.runtime.applied != AppliedState::Failed {
            self.epoch = self.epoch.saturating_add(1);
            self.completed = None;
            self.safe_to_play = false;
            mailbox.runtime.applied = AppliedState::Failed;
            mailbox.runtime.reason = Some("native audio filter failed or was disabled".to_owned());
            mailbox.runtime.effective_preamp_db = None;
            drop(mailbox);
            self.shared.0 .1.notify_all();
        }
    }

    pub(super) fn requested(&self) -> bool {
        self.shared
            .0
             .0
            .lock()
            .expect("effect mailbox poisoned")
            .target
            .as_ref()
            .is_some_and(|target| target.enabled)
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_cmp
    )]
    pub(super) fn property(&mut self, name: &str, value: f64) -> bool {
        match name {
            "audio-params/samplerate" => {
                self.sample_rate = if value.is_finite()
                    && value >= 1.0
                    && value <= f64::from(u32::MAX)
                    && value.fract() == 0.0
                {
                    Some(value as u32)
                } else {
                    None
                };
            }
            "audio-params/channel-count" => {
                self.channels = Some(if value == 1.0 {
                    ChannelLayout::Mono
                } else if value == 2.0 {
                    ChannelLayout::Stereo
                } else {
                    ChannelLayout::Other
                });
            }
            "audio-out-params/samplerate" | "audio-out-params/channel-count" => return true,
            _ => return false,
        }
        let environment =
            self.sample_rate
                .zip(self.channels)
                .map(|(sample_rate, channel_layout)| ProcessingEnvironment {
                    sample_rate,
                    channel_layout,
                });
        if environment != self.environment {
            self.epoch = self.epoch.saturating_add(1);
            self.environment = environment;
            self.completed = None;
        }
        true
    }

    /// Returns whether it is safe for a transport operation to release pause.
    #[allow(clippy::significant_drop_tightening)] // Keep the mailbox revision stable while applying its native request.
    pub(super) fn tick<B: Backend>(
        &mut self,
        backend: &mut B,
        playing: bool,
        _force: bool,
    ) -> bool {
        let shared = self.shared.clone();
        let mut mailbox = shared.0 .0.lock().expect("effect mailbox poisoned");
        let Some(target) = mailbox.target.clone() else {
            return true;
        };
        mailbox.runtime.playback_epoch = self.epoch;
        if self.completed == Some((target.revision, self.epoch)) {
            return self.safe_to_play;
        }
        if target.enabled && (!playing || self.environment.is_none()) {
            if mailbox.runtime.applied != AppliedState::Failed {
                mailbox.runtime.applied = AppliedState::Pending;
            }
            return false;
        }
        if !target.enabled {
            let result = backend.bypass_effects();
            return self.finish(&mut mailbox, &target, result, None);
        }
        let environment = self.environment.expect("environment checked");
        let analyzed = match &self.analysis {
            Some((revision, epoch, analysis))
                if *revision == target.revision && *epoch == self.epoch =>
            {
                Ok(analysis.clone())
            }
            _ => crate::effects::math::analyze(&target.payload, environment),
        };
        match analyzed {
            Ok(analysis) => {
                self.analysis = Some((target.revision, self.epoch, analysis.clone()));
                let result = backend.apply_effects(&target.payload, environment, &analysis);
                let failed = result.is_err();
                let safe = !failed || matches!(backend.bypass_effects(), Ok(true));
                let ready = self.finish(
                    &mut mailbox,
                    &target,
                    result,
                    Some((&environment, &analysis)),
                );
                if failed {
                    self.safe_to_play = safe;
                }
                ready || (failed && safe)
            }
            Err(error) => {
                // Unsupported layouts keep the ordinary transport usable, with
                // explicit unavailability and confirmed bypass rather than a downmix.
                mailbox.runtime.reason = Some(error.to_string());
                mailbox.runtime.processing_rate = Some(environment.sample_rate);
                mailbox.runtime.channel_layout = Some(environment.channel_layout);
                match backend.bypass_effects() {
                    Ok(false) => {
                        // A smooth bypass may need several actor ticks before
                        // the old chain is removed. Keep driving it while pause
                        // still guards the first output sample.
                        mailbox.runtime.applied = AppliedState::Pending;
                        self.safe_to_play = false;
                        return false;
                    }
                    Ok(true) => {
                        mailbox.runtime.applied = AppliedState::Unavailable;
                        mailbox.runtime.effective_preamp_db = Some(0.0);
                        mailbox.runtime.active_bands = [false; 10];
                        self.safe_to_play = true;
                    }
                    Err(message) => {
                        mailbox.runtime.applied = AppliedState::Failed;
                        mailbox.runtime.effective_preamp_db = None;
                        mailbox.runtime.active_bands = [false; 10];
                        mailbox.runtime.reason = Some(format!("{error}; bypass failed: {message}"));
                        self.safe_to_play = false;
                    }
                }
                self.completed = Some((target.revision, self.epoch));
                self.shared.0 .1.notify_all();
                self.safe_to_play
            }
        }
    }

    fn finish(
        &mut self,
        mailbox: &mut Mailbox,
        target: &Target,
        result: Result<bool, String>,
        analysis: Option<(
            &ProcessingEnvironment,
            &crate::effects::math::EffectsAnalysis,
        )>,
    ) -> bool {
        match result {
            Ok(false) => return false,
            Ok(true) => {
                mailbox.runtime.applied = if target.enabled {
                    AppliedState::Applied
                } else {
                    AppliedState::Bypassed
                };
                mailbox.runtime.effective_preamp_db = Some(
                    analysis
                        .as_ref()
                        .map_or(0.0, |(_, analysis)| analysis.effective_preamp_db),
                );
                mailbox.runtime.active_bands = analysis
                    .as_ref()
                    .map_or([false; 10], |(_, analysis)| analysis.active_bands);
                if let Some((environment, _)) = analysis {
                    mailbox.runtime.processing_rate = Some(environment.sample_rate);
                    mailbox.runtime.channel_layout = Some(environment.channel_layout);
                }
                mailbox.runtime.reason = None;
            }
            Err(message) => {
                tracing::warn!(
                    revision = target.revision,
                    playback_epoch = self.epoch,
                    error = %message,
                    "audio effect request was not applied"
                );
                mailbox.runtime.applied = AppliedState::Failed;
                mailbox.runtime.reason = Some(message);
                mailbox.runtime.effective_preamp_db = None;
            }
        }
        self.completed = Some((target.revision, self.epoch));
        self.shared.0 .1.notify_all();
        self.safe_to_play = mailbox.runtime.applied != AppliedState::Failed;
        self.safe_to_play
    }
}

#[cfg(test)]
#[path = "actor_effects_tests.rs"]
mod tests;
