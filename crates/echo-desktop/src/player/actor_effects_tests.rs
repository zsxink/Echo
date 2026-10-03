use super::super::BackendEvent;
use super::*;
use std::collections::VecDeque;
use std::path::Path;

#[derive(Default)]
struct Driver {
    applies: Vec<Payload>,
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
    ) -> Result<bool, String> {
        self.applies.push(payload.clone());
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
