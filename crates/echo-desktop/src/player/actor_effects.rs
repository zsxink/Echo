//! Latest-intent mailbox and actor-thread effect scheduling.
//! Backend calls execute while the mailbox lock is held: a superseding request
//! can never be accepted between the revision check and the corresponding write.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use echo_core::domain::media::LoudnessTags;

use super::super::port::PlayerError;
use super::{Backend, BackendProperty};
use crate::effects::loudness::{
    self, NormalizationGain, NormalizationPreference, NormalizationRuntime,
};
use crate::effects::{
    AppliedState, ChannelLayout, EffectsRuntime, EqCurve, Payload, ProcessingEnvironment,
};

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
    /// The independent normalization preference lives beside the effects
    /// mailbox so a change invalidates the actor's applied-configuration key.
    normalization: NormalizationPreference,
    normalization_runtime: NormalizationRuntime,
    normalization_revision: u64,
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

    /// Publish a validated normalization preference. A neutral effects target
    /// is created when none exists, so a normalization-only session still has
    /// a configuration the actor can apply through the shared gain stage.
    pub(super) fn set_normalization(
        &self,
        preference: NormalizationPreference,
    ) -> Result<(), PlayerError> {
        preference
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
        if mailbox.target.is_none() {
            mailbox.target = Some(Target {
                revision: mailbox.runtime.revision,
                enabled: false,
                payload: Payload::default(),
            });
        }
        mailbox.normalization = preference;
        mailbox.normalization_revision = mailbox.normalization_revision.saturating_add(1);
        if !preference.enabled {
            mailbox.normalization_runtime = NormalizationRuntime::default();
        }
        drop(mailbox);
        Ok(())
    }

    pub(super) fn normalization_runtime(&self) -> NormalizationRuntime {
        self.0
             .0
            .lock()
            .expect("effect mailbox poisoned")
            .normalization_runtime
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
    completed: Option<(u64, u64, u64, u64)>,
    analysis: Option<(u64, u64, crate::effects::math::EffectsAnalysis)>,
    safe_to_play: bool,
    /// Loudness tags of the currently loaded library song. Temporary items
    /// and lookup failures stay `None` (0 dB).
    tags: Option<LoudnessTags>,
    tags_revision: u64,
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
            tags: None,
            tags_revision: 0,
        }
    }

    /// Replace the current song's loudness tags and invalidate the applied
    /// configuration so the new gain is computed before the first sample.
    pub(super) fn set_tags(&mut self, tags: Option<LoudnessTags>) {
        if self.tags != tags {
            self.tags = tags;
            self.tags_revision = self.tags_revision.saturating_add(1);
            self.completed = None;
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
            let effects_enabled = mailbox.target.as_ref().is_some_and(|target| target.enabled);
            // The pause barrier covers the normalization-only gain stage too.
            let enabled = effects_enabled || mailbox.normalization.enabled;
            if effects_enabled {
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
        let mailbox = self.shared.0 .0.lock().expect("effect mailbox poisoned");
        mailbox.target.as_ref().is_some_and(|target| target.enabled)
            || mailbox.normalization.enabled
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
    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
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
        // The applied key includes normalization: a preference or a new song's
        // tags must recompute the gain even when the effects request is old.
        let key = (
            target.revision,
            self.epoch,
            mailbox.normalization_revision,
            self.tags_revision,
        );
        if self.completed == Some(key) {
            return self.safe_to_play;
        }
        let normalization_enabled = mailbox.normalization.enabled;
        let effects_enabled = target.enabled;
        if !effects_enabled && !normalization_enabled {
            mailbox.normalization_runtime = NormalizationRuntime::default();
            let result = backend.bypass_effects();
            return self.finish(&mut mailbox, &target, key, result, None, None);
        }
        if !playing || self.environment.is_none() {
            if effects_enabled && mailbox.runtime.applied != AppliedState::Failed {
                mailbox.runtime.applied = AppliedState::Pending;
            }
            return false;
        }
        let environment = self.environment.expect("environment checked");
        // With effects bypassed, the gain stage uses a neutral EQ analysis and
        // installs no equalizer/extrastereo nodes (see native_effects.rs).
        let gain_only = !effects_enabled;
        let payload = if gain_only {
            Payload::Eq(EqCurve::default())
        } else {
            target.payload.clone()
        };
        let analyzed = match &self.analysis {
            Some((revision, epoch, analysis))
                if *revision == target.revision && *epoch == self.epoch && !gain_only =>
            {
                Ok(analysis.clone())
            }
            _ => crate::effects::math::analyze(&payload, environment),
        };
        match analyzed {
            Ok(analysis) => {
                if !gain_only {
                    self.analysis = Some((target.revision, self.epoch, analysis.clone()));
                }
                let normalization = loudness::compute(self.tags, &mailbox.normalization, &analysis)
                    .unwrap_or_else(|error| {
                        tracing::warn!(error = %error, "invalid normalization preference");
                        NormalizationGain {
                            gain_db: 0.0,
                            combined_preamp_db: analysis.effective_preamp_db,
                            status: loudness::NormalizationStatus::Disabled,
                            source: None,
                        }
                    });
                let result = backend.apply_effects(
                    &payload,
                    environment,
                    &analysis,
                    normalization.combined_preamp_db,
                    gain_only,
                );
                let failed = result.is_err();
                let safe = !failed || matches!(backend.bypass_effects(), Ok(true));
                let ready = self.finish(
                    &mut mailbox,
                    &target,
                    key,
                    result,
                    Some((&environment, &analysis)),
                    Some(normalization),
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
                mailbox.normalization_runtime = NormalizationRuntime {
                    reason: Some(error.to_string()),
                    ..NormalizationRuntime::default()
                };
                match backend.bypass_effects() {
                    Ok(false) => {
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
                self.completed = Some(key);
                self.shared.0 .1.notify_all();
                self.safe_to_play
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        mailbox: &mut Mailbox,
        target: &Target,
        key: (u64, u64, u64, u64),
        result: Result<bool, String>,
        analysis: Option<(
            &ProcessingEnvironment,
            &crate::effects::math::EffectsAnalysis,
        )>,
        normalization: Option<NormalizationGain>,
    ) -> bool {
        match result {
            Ok(false) => return false,
            Ok(true) => {
                mailbox.runtime.applied = if target.enabled {
                    AppliedState::Applied
                } else {
                    // A gain-only stage must never look like an applied preset.
                    AppliedState::Bypassed
                };
                let effective = if target.enabled {
                    analysis
                        .as_ref()
                        .map_or(0.0, |(_, analysis)| analysis.effective_preamp_db)
                } else {
                    0.0
                };
                mailbox.runtime.effective_preamp_db = Some(effective);
                mailbox.runtime.active_bands = if target.enabled {
                    analysis
                        .as_ref()
                        .map_or([false; 10], |(_, analysis)| analysis.active_bands)
                } else {
                    [false; 10]
                };
                if let Some((environment, _)) = analysis {
                    mailbox.runtime.processing_rate = Some(environment.sample_rate);
                    mailbox.runtime.channel_layout = Some(environment.channel_layout);
                }
                mailbox.runtime.reason = None;
                mailbox.normalization_runtime = normalization
                    .as_ref()
                    .map_or_else(NormalizationRuntime::default, NormalizationRuntime::from);
            }
            Err(message) => {
                tracing::warn!(
                    revision = target.revision,
                    playback_epoch = self.epoch,
                    error = %message,
                    "audio effect request was not applied"
                );
                mailbox.runtime.applied = AppliedState::Failed;
                mailbox.runtime.reason = Some(message.clone());
                mailbox.runtime.effective_preamp_db = None;
                mailbox.normalization_runtime = normalization
                    .as_ref()
                    .map_or_else(NormalizationRuntime::default, NormalizationRuntime::from);
                mailbox.normalization_runtime.reason = Some(message);
            }
        }
        self.completed = Some(key);
        self.shared.0 .1.notify_all();
        self.safe_to_play = mailbox.runtime.applied != AppliedState::Failed;
        self.safe_to_play
    }
}

#[cfg(test)]
#[path = "actor_effects_tests.rs"]
mod tests;
