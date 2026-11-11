use super::super::BackendEvent;
use super::*;
use std::collections::VecDeque;
use std::path::Path;

#[derive(Default)]
struct Driver {
    applies: Vec<Payload>,
    /// (combined preamp dB, gain-only) for every applied configuration.
    applied_gains: Vec<(f64, bool)>,
    bypasses: usize,
    properties: Vec<BackendProperty>,
    fail_apply: bool,
    fail_bypass: bool,
    bypass_results: VecDeque<Result<bool, String>>,
}
impl Backend for Driver {
    fn pump(&mut self) -> Option<BackendEvent> {
        None
    }
    fn queue_load(&mut self, _: &Path) {}
    fn observe_continuous(&mut self) {}
    fn write_property(&mut self, property: BackendProperty) -> bool {
        self.properties.push(property);
        true
    }
    fn terminate(&mut self) {}
    fn apply_effects(
        &mut self,
        payload: &Payload,
        _: ProcessingEnvironment,
        _: &crate::effects::math::EffectsAnalysis,
        combined_preamp_db: f64,
        gain_only: bool,
    ) -> Result<bool, String> {
        self.applies.push(payload.clone());
        self.applied_gains.push((combined_preamp_db, gain_only));
        if self.fail_apply {
            Err("injected effect failure".to_owned())
        } else {
            Ok(true)
        }
    }
    fn bypass_effects(&mut self) -> Result<bool, String> {
        self.bypasses += 1;
        if let Some(result) = self.bypass_results.pop_front() {
            return result;
        }
        if self.fail_bypass {
            Err("injected bypass failure".to_owned())
        } else {
            Ok(true)
        }
    }
}
fn environment(engine: &mut Engine, sample_rate: f64, channels: f64) {
    assert!(engine.property("audio-params/samplerate", sample_rate));
    assert!(engine.property("audio-params/channel-count", channels));
}
#[test]
fn pending_is_not_applied_without_observed_environment_or_while_paused() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.submit(1, true, Payload::default()).unwrap();
    assert!(!engine.tick(&mut driver, true, true));
    environment(&mut engine, 48_000.0, 2.0);
    assert!(!engine.tick(&mut driver, false, true));
    assert_eq!(shared.runtime().applied, AppliedState::Pending);
    assert!(driver.applies.is_empty());
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Applied);
    assert_eq!(shared.runtime().processing_rate, Some(48_000));
}

#[test]
fn configured_node_failure_is_visible_while_paused_and_does_not_repeat_installation() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.submit(1, true, Payload::default()).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, true));
    driver.fail_apply = true;
    engine.configuration_failed();
    assert!(!engine.tick(&mut driver, false, false));
    assert_eq!(shared.runtime().applied, AppliedState::Failed);
    assert!(shared
        .runtime()
        .reason
        .unwrap()
        .contains("failed or was disabled"));
    assert_eq!(
        driver.applies.len(),
        1,
        "paused failure must not restore an enabled graph"
    );

    assert!(engine.tick(&mut driver, true, false));
    let count = driver.applies.len();
    // Multiple native errors from one configuration, possibly queued before
    // cleanup, must not discard its completed failure receipt.
    engine.configuration_failed();
    assert!(engine.tick(&mut driver, true, false));
    assert_eq!(driver.applies.len(), count);
    assert_eq!(shared.runtime().applied, AppliedState::Failed);
}

#[test]
fn actor_applies_the_latest_value_on_its_next_tick_and_disable_cancels_unsent_edits() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    environment(&mut engine, 48_000.0, 2.0);
    shared.submit(1, true, Payload::default()).unwrap();
    let mut curve = crate::effects::EqCurve::default();
    curve.gains_db[4] = 2.0;
    let last = Payload::Eq(curve);
    shared.submit(2, true, last.clone()).unwrap();
    assert!(engine.tick(&mut driver, true, false));
    assert_eq!(driver.applies, [last]);
    shared.submit(3, true, Payload::default()).unwrap();
    shared.submit(4, false, Payload::default()).unwrap();
    assert!(engine.tick(&mut driver, true, false));
    assert_eq!(driver.applies.len(), 1);
    assert_eq!(shared.runtime().revision, 4);
    assert_eq!(shared.runtime().applied, AppliedState::Bypassed);
}

#[test]
fn stale_requests_and_invalid_payload_cannot_replace_the_latest_target() {
    let shared = Shared::default();
    shared.submit(5, false, Payload::default()).unwrap();
    assert!(shared.submit(4, true, Payload::default()).is_err());
    let mut curve = crate::effects::EqCurve::default();
    curve.gains_db[0] = f64::NAN;
    assert!(shared.submit(6, true, Payload::Eq(curve)).is_err());
    assert_eq!(shared.runtime().revision, 5);
}

#[test]
fn load_holds_first_sample_and_new_environment_advances_epoch() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.submit(1, true, Payload::default()).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, true));
    let first = shared.runtime().playback_epoch;
    engine.begin_load(&mut driver);
    assert_eq!(
        driver.properties.last(),
        Some(&BackendProperty::Pause(true))
    );
    assert!(!engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Pending);
    environment(&mut engine, 22_050.0, 1.0);
    assert!(engine.tick(&mut driver, true, true));
    let runtime = shared.runtime();
    assert!(runtime.playback_epoch > first);
    assert_eq!(runtime.processing_rate, Some(22_050));
    assert!(!runtime.active_bands[9]);
}

#[test]
fn failure_confirms_bypass_and_never_restores_an_old_enabled_revision() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver {
        fail_apply: true,
        ..Default::default()
    };
    environment(&mut engine, 48_000.0, 2.0);
    shared.submit(1, true, Payload::default()).unwrap();
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Failed);
    assert_eq!(driver.bypasses, 1);
    shared.submit(2, false, Payload::default()).unwrap();
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Bypassed);
    assert_eq!(driver.applies.len(), 1);
}

#[test]
fn inability_to_bypass_keeps_the_first_sample_barrier_closed() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver {
        fail_apply: true,
        fail_bypass: true,
        ..Default::default()
    };
    environment(&mut engine, 48_000.0, 2.0);
    shared.submit(1, true, Payload::default()).unwrap();
    assert!(!engine.tick(&mut driver, true, true));
    assert!(!engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Failed);
}

#[test]
fn mono_spatial_is_unavailable_with_confirmed_bypass() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    environment(&mut engine, 48_000.0, 1.0);
    shared
        .submit(
            1,
            true,
            Payload::Spatial(crate::effects::Spatial::default()),
        )
        .unwrap();
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(shared.runtime().applied, AppliedState::Unavailable);
    assert!(driver.applies.is_empty());
}

#[test]
fn unsupported_environment_waits_for_bypass_then_caches_its_confirmed_safety() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    environment(&mut engine, 48_000.0, 2.0);
    let mut curve = crate::effects::EqCurve::default();
    curve.gains_db[4] = 6.0;
    shared.submit(1, true, Payload::Eq(curve)).unwrap();
    assert!(engine.tick(&mut driver, true, false));
    assert!(shared.runtime().active_bands.iter().any(|active| *active));
    assert_ne!(shared.runtime().effective_preamp_db, Some(0.0));

    shared
        .submit(
            2,
            true,
            Payload::Spatial(crate::effects::Spatial::default()),
        )
        .unwrap();
    environment(&mut engine, 48_000.0, 1.0);
    driver.bypass_results = [Ok(false), Ok(false), Ok(true)].into();
    for expected_calls in 1..=2 {
        assert!(!engine.tick(&mut driver, true, false));
        assert_eq!(shared.runtime().applied, AppliedState::Pending);
        assert_eq!(driver.bypasses, expected_calls);
    }
    assert!(engine.tick(&mut driver, true, false));
    let runtime = shared.runtime();
    assert_eq!(runtime.applied, AppliedState::Unavailable);
    assert_eq!(runtime.effective_preamp_db, Some(0.0));
    assert_eq!(runtime.active_bands, [false; 10]);
    assert_eq!(runtime.processing_rate, Some(48_000));
    assert_eq!(runtime.channel_layout, Some(ChannelLayout::Mono));
    assert!(runtime.reason.is_some());
    assert!(engine.tick(&mut driver, true, false));
    assert_eq!(driver.bypasses, 3);

    // The request survives temporary unavailability and is confirmed again
    // when the observed input returns to a supported layout.
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, false));
    assert_eq!(shared.runtime().applied, AppliedState::Applied);
    assert_eq!(shared.runtime().revision, 2);
    assert_eq!(driver.applies.len(), 2);
}

#[test]
fn unsupported_environment_bypass_failure_stays_unsafe_on_later_ticks() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver {
        fail_bypass: true,
        ..Default::default()
    };
    environment(&mut engine, 48_000.0, 1.0);
    shared
        .submit(
            1,
            true,
            Payload::Spatial(crate::effects::Spatial::default()),
        )
        .unwrap();
    for _ in 0..3 {
        assert!(!engine.tick(&mut driver, true, false));
        assert_eq!(shared.runtime().applied, AppliedState::Failed);
    }
    assert_eq!(driver.bypasses, 1);
    assert!(driver.applies.is_empty());
}

#[test]
fn confirmed_unavailable_bypass_reopens_a_previously_failed_playback_barrier() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver {
        fail_bypass: true,
        ..Default::default()
    };
    shared.submit(1, false, Payload::default()).unwrap();
    assert!(!engine.tick(&mut driver, true, false));
    assert_eq!(shared.runtime().applied, AppliedState::Failed);

    driver.fail_bypass = false;
    environment(&mut engine, 48_000.0, 1.0);
    shared
        .submit(
            2,
            true,
            Payload::Spatial(crate::effects::Spatial::default()),
        )
        .unwrap();
    for _ in 0..3 {
        assert!(engine.tick(&mut driver, true, false));
        assert_eq!(shared.runtime().applied, AppliedState::Unavailable);
    }
    assert_eq!(driver.bypasses, 2);
}

#[test]
fn synchronous_bypass_waits_for_the_actor_and_shutdown_wakes_waiters() {
    let shared = Shared::default();
    let waiting = shared.clone();
    let thread = std::thread::spawn(move || waiting.bypass(1));
    while shared.runtime().revision == 0 {
        std::thread::yield_now();
    }
    let mut engine = Engine::new(shared.clone());
    assert!(engine.tick(&mut Driver::default(), false, false));
    assert_eq!(
        thread.join().unwrap().unwrap().applied,
        AppliedState::Bypassed
    );
    shared.close();
    assert!(shared.submit(2, false, Payload::default()).is_err());
}

// ---------------------------------------------------------------------------
// Loudness normalization (issue #101): independent gain layer
// ---------------------------------------------------------------------------

use crate::effects::loudness::{NormalizationMode, NormalizationPreference, NormalizationStatus};
use echo_core::domain::media::{LoudnessSource, LoudnessTags};

fn enabled_preference(preamp_db: f64) -> NormalizationPreference {
    NormalizationPreference {
        enabled: true,
        mode: NormalizationMode::Track,
        preamp_db,
        ..NormalizationPreference::default()
    }
}

#[test]
fn normalization_only_session_holds_first_sample_and_bypasses_the_preset_state() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.set_normalization(enabled_preference(0.0)).unwrap();
    // No effects target was submitted: the neutral target created by
    // set_normalization still drives the pause barrier and the gain stage.
    engine.begin_load(&mut driver);
    assert_eq!(
        driver.properties.last(),
        Some(&BackendProperty::Pause(true)),
        "gain-only stage must guard the first sample"
    );
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(driver.applies.len(), 1);
    assert_eq!(driver.applied_gains.last(), Some(&(0.0, true)));
    assert_eq!(shared.runtime().applied, AppliedState::Bypassed);
    assert_eq!(
        shared.normalization_runtime().status,
        NormalizationStatus::NoTags
    );
}

#[test]
fn tagged_track_gain_reaches_the_backend_and_is_not_reported_as_effects_applied() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.set_normalization(enabled_preference(0.0)).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    engine.set_tags(Some(LoudnessTags::new(
        LoudnessSource::ReplayGain2,
        Some(-6_000),
        None,
        Some(900_000),
        None,
    )));
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(driver.applied_gains.last(), Some(&(-6.0, true)));
    assert_eq!(shared.runtime().applied, AppliedState::Bypassed);
    let runtime = shared.normalization_runtime();
    assert_eq!(runtime.status, NormalizationStatus::Applied);
    assert!((runtime.gain_db - (-6.0)).abs() < 1e-9);
}

#[test]
fn tag_and_preference_changes_invalidate_the_applied_configuration() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.set_normalization(enabled_preference(0.0)).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    let tags = LoudnessTags::new(LoudnessSource::ReplayGain2, Some(-4_000), None, None, None);
    engine.set_tags(Some(tags));
    assert!(engine.tick(&mut driver, true, true));
    assert!(
        engine.tick(&mut driver, true, true),
        "unchanged config is cached"
    );
    assert_eq!(driver.applies.len(), 1);

    engine.set_tags(Some(LoudnessTags::new(
        LoudnessSource::R128,
        Some(-2_000),
        None,
        None,
        None,
    )));
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(driver.applies.len(), 2, "new track recomputes its gain");
    assert_eq!(driver.applied_gains.last(), Some(&(3.0, true)));

    shared.set_normalization(enabled_preference(1.0)).unwrap();
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(
        driver.applies.len(),
        3,
        "preference change recomputes its gain"
    );
    assert_eq!(driver.applied_gains.last(), Some(&(4.0, true)));
}

#[test]
fn disabling_normalization_bypasses_without_leaving_a_gain_stage() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver::default();
    shared.set_normalization(enabled_preference(0.0)).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, true));
    shared
        .set_normalization(NormalizationPreference::default())
        .unwrap();
    assert!(engine.tick(&mut driver, true, true));
    assert_eq!(driver.bypasses, 1);
    assert_eq!(shared.runtime().applied, AppliedState::Bypassed);
    assert_eq!(
        shared.normalization_runtime().status,
        NormalizationStatus::Disabled
    );
}

#[test]
fn failed_native_configuration_degrades_to_zero_db_without_stopping_playback() {
    let shared = Shared::default();
    let mut engine = Engine::new(shared.clone());
    let mut driver = Driver {
        fail_apply: true,
        ..Driver::default()
    };
    shared.set_normalization(enabled_preference(0.0)).unwrap();
    environment(&mut engine, 48_000.0, 2.0);
    assert!(engine.tick(&mut driver, true, true));
    assert!(shared.normalization_runtime().reason.is_some());
    assert_eq!(shared.runtime().applied, AppliedState::Failed);
    assert_eq!(driver.bypasses, 1, "failure confirms a safe bypass");
}

#[test]
fn invalid_preference_is_rejected_by_the_mailbox() {
    let shared = Shared::default();
    assert!(shared
        .set_normalization(NormalizationPreference {
            preamp_db: 0.25,
            ..enabled_preference(0.0)
        })
        .is_err());
    assert_eq!(
        shared.normalization_runtime().status,
        NormalizationStatus::Disabled
    );
}
