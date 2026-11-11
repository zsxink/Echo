//! Loudness-normalization gain-stage tests (issue #101), split from
//! `native_effects_tests.rs` to keep each source file under the 1000-line gate.

#![allow(clippy::float_cmp)] // Some neutral-value assertions compare exact f64 literals.

use super::tests::{complete, environment, halfway};
use super::*;
use crate::effects::EqCurve;

// ---------------------------------------------------------------------------
// Loudness normalization gain stage (issue #101)
// ---------------------------------------------------------------------------

fn neutral_analysis() -> EffectsAnalysis {
    let payload = Payload::Eq(EqCurve::default());
    crate::effects::math::analyze(&payload, environment()).unwrap()
}

fn capture(commands: &mut Vec<Vec<String>>) -> impl FnMut(&[String]) -> Result<(), String> + '_ {
    move |args| {
        commands.push(args.to_vec());
        Ok(())
    }
}

/// Simulate mpv's `AUDIO_RECONFIG` + node confirmation without discarding the
/// in-flight ramp (unlike `reconfirm()`, which models a graph rebuild).
fn confirm(native: &mut NativeEffects) {
    native.reconfiguration_observed = true;
    native.configured();
}

#[test]
fn gain_only_installs_no_equalizer_or_spatial_node() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let analysis = neutral_analysis();
    let payload = Payload::Eq(EqCurve::default());
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            -3.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    let added = commands
        .iter()
        .find(|args| {
            args.first().map(String::as_str) == Some("af")
                && args.get(1).map(String::as_str) == Some("add")
        })
        .expect("af add installs the gain stage");
    let graph = &added[2];
    assert!(graph.contains("@echo_float"), "{graph}");
    assert!(graph.contains("@echo_preamp"), "{graph}");
    assert!(graph.contains("@echo_limiter"), "{graph}");
    assert!(!graph.contains("equalizer"), "{graph}");
    assert!(!graph.contains("extrastereo"), "{graph}");
    assert_eq!(native.labels, ["echo_float", "echo_preamp", "echo_limiter"]);
}

#[test]
fn positive_normalization_gain_is_not_cancelled_by_the_eq_protection_floor() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let analysis = neutral_analysis();
    let payload = Payload::Eq(EqCurve::default());
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            6.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    confirm(&mut native);
    complete(&mut native);
    assert!(native
        .apply(
            &payload,
            environment(),
            &analysis,
            6.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    let preamp_values: Vec<f64> = commands
        .iter()
        .filter(|args| {
            args.first().map(String::as_str) == Some("af-command")
                && args.get(1).map(String::as_str) == Some("echo_preamp")
        })
        .filter_map(|args| args.last().and_then(|value| value.parse().ok()))
        .collect();
    let expected = 10.0_f64.powf(6.0 / 20.0);
    assert!(
        preamp_values
            .iter()
            .any(|value| (value - expected).abs() < 1e-9),
        "combined +6 dB must reach the volume node: {preamp_values:?}"
    );
}

#[test]
fn track_gain_change_ramp_reaches_the_new_target_without_a_hard_cut() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let analysis = neutral_analysis();
    let payload = Payload::Eq(EqCurve::default());
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            -6.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    confirm(&mut native);
    complete(&mut native);
    assert!(native
        .apply(
            &payload,
            environment(),
            &analysis,
            -6.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    commands.clear();

    // A new track asks for a different gain; the same graph is reused and the
    // gain is interpolated from the previous value.
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            2.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    // Drive one midpoint tick, then the final tick; neither depends on how
    // much wall time happened to pass between calls.
    halfway(&mut native);
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            2.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    complete(&mut native);
    assert!(native
        .apply(
            &payload,
            environment(),
            &analysis,
            2.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    let preamp_values: Vec<f64> = commands
        .iter()
        .filter(|args| {
            args.first().map(String::as_str) == Some("af-command")
                && args.get(1).map(String::as_str) == Some("echo_preamp")
        })
        .filter_map(|args| args.last().and_then(|value| value.parse().ok()))
        .collect();
    let from = 10.0_f64.powf(-6.0 / 20.0);
    let target = 10.0_f64.powf(2.0 / 20.0);
    assert!(
        preamp_values.len() >= 2,
        "an intermediate gain proves the ramp: {preamp_values:?}"
    );
    assert!(
        preamp_values
            .iter()
            .any(|value| *value > from && *value < target),
        "intermediate gain between old and new track: {preamp_values:?}"
    );
    assert!((preamp_values.last().unwrap() - target).abs() < 1e-9);
    assert!(
        !commands
            .iter()
            .any(|args| args.first().map(String::as_str) == Some("af")
                && args.get(1).map(String::as_str) == Some("add")),
        "a gain change must not rebuild the graph"
    );
}

#[test]
fn disabling_everything_removes_the_gain_stage() {
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    let analysis = neutral_analysis();
    let payload = Payload::Eq(EqCurve::default());
    assert!(!native
        .apply(
            &payload,
            environment(),
            &analysis,
            -3.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    confirm(&mut native);
    complete(&mut native);
    assert!(native
        .apply(
            &payload,
            environment(),
            &analysis,
            -3.0,
            true,
            capture(&mut commands)
        )
        .unwrap());
    commands.clear();
    assert!(!native.bypass(capture(&mut commands)).unwrap());
    complete(&mut native);
    assert!(native.bypass(capture(&mut commands)).unwrap());
    assert!(native.labels.is_empty(), "no filter nodes remain");
    assert!(commands
        .iter()
        .any(|args| args.first().map(String::as_str) == Some("af")
            && args.get(1).map(String::as_str) == Some("remove")));
}
