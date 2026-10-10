//! Regression tests for audible-transition budgets and command ordering.
#![allow(clippy::float_cmp)] // Fixtures require exact neutral and endpoint values.

use super::*;
use crate::effects::{
    math::analyze, presets, ChannelLayout, EqCurve, PreampMode, PresetId, Spatial,
};

fn environment(rate: u32) -> ProcessingEnvironment {
    ProcessingEnvironment {
        sample_rate: rate,
        channel_layout: ChannelLayout::Stereo,
    }
}

fn settled(payload: &Payload, rate: u32) -> NativeEffects {
    let mut native = NativeEffects::default();
    let analysis = analyze(payload, environment(rate)).unwrap();
    native
        .apply(payload, environment(rate), &analysis, |_| Ok(()))
        .unwrap();
    native.reconfirm();
    native.configured();
    native
        .apply(payload, environment(rate), &analysis, |_| Ok(()))
        .unwrap();
    advance_to(&mut native, 1.0, &mut |_| Ok(()));
    native
}

fn advance_to(native: &mut NativeEffects, fraction: f64, command: &mut Command<'_>) {
    native.ramp.as_mut().unwrap().started = Instant::now()
        .checked_sub(TRANSITION.mul_f64(fraction))
        .unwrap();
    native.advance(command).unwrap();
}

#[test]
fn initial_eq_installs_neutral_preamp_before_the_ramp() {
    let mut curve = EqCurve::default();
    curve.gains_db[5] = 12.0;
    let payload = Payload::Eq(curve);
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    let mut native = NativeEffects::default();
    let mut commands = Vec::new();
    native
        .apply(&payload, environment(48_000), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    assert!(commands[0][2].contains("volume@preamp=volume=1:precision=double"));
    assert_eq!(native.current.as_ref().unwrap().preamp, 0.0);
}

#[test]
fn rock_reconfirmation_preserves_preamp_without_positive_gain_sum_dip() {
    let payload = presets::builtin(&PresetId::builtin("rock"))
        .unwrap()
        .payload;
    let mut native = settled(&payload, 48_000);
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    let before = native.current.as_ref().unwrap().preamp;
    let mut commands = Vec::new();
    native.reconfirm();
    native.configured();
    native
        .apply(&payload, environment(48_000), &analysis, |args| {
            commands.push(args.to_vec());
            Ok(())
        })
        .unwrap();
    advance_to(&mut native, 0.5, &mut |args| {
        commands.push(args.to_vec());
        Ok(())
    });
    assert!((native.current.as_ref().unwrap().preamp - before).abs() < 0.000_001);
    for args in commands.iter().filter(|args| args[1] == "echo_preamp") {
        let actual = 20.0 * args[3].parse::<f64>().unwrap().log10();
        assert!((actual - before).abs() < 0.000_001);
    }
}

#[test]
fn rebuilt_graph_reasserts_protection_before_restoring_eq() {
    let payload = presets::builtin(&PresetId::builtin("rock"))
        .unwrap()
        .payload;
    let mut native = settled(&payload, 48_000);
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    // Model loadfile rebuilding every native node with its installation values.
    let mut backend_gains = [0.0; 10];
    let mut backend_preamp = 0.0;
    let mut commands = Vec::new();
    native.reconfirm();
    native.configured();
    native
        .apply(&payload, environment(48_000), &analysis, |args| {
            commands.push(args.to_vec());
            if args[1] == "echo_preamp" {
                backend_preamp = 20.0 * args[3].parse::<f64>().unwrap().log10();
            } else if let Some(index) = args[1].strip_prefix("echo_eq") {
                backend_gains[index.parse::<usize>().unwrap()] = args[3].parse().unwrap();
                if backend_gains[index.parse::<usize>().unwrap()] > 0.0 {
                    assert!(
                        backend_preamp < -4.0,
                        "protection must precede restored boosts"
                    );
                }
            }
            Ok(())
        })
        .unwrap();
    assert!(
        commands
            .iter()
            .position(|args| args[1] == "echo_preamp")
            .unwrap()
            < commands
                .iter()
                .position(|args| args[1] == "echo_eq0")
                .unwrap()
    );
    assert!((backend_preamp - analysis.effective_preamp_db).abs() < 0.000_001);
    let Payload::Eq(curve) = payload else {
        unreachable!()
    };
    assert_eq!(backend_gains, curve.gains_db);
}

#[test]
fn manual_preamp_edit_is_interpolated_before_the_final_tick() {
    let curve = EqCurve {
        preamp_mode: PreampMode::Manual,
        requested_preamp_db: -12.0,
        ..EqCurve::default()
    };
    let mut native = settled(&Payload::Eq(curve.clone()), 48_000);
    let payload = Payload::Eq(EqCurve {
        requested_preamp_db: -6.0,
        ..curve
    });
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    native
        .apply(&payload, environment(48_000), &analysis, |_| Ok(()))
        .unwrap();
    advance_to(&mut native, 0.5, &mut |_| Ok(()));
    assert!((native.current.as_ref().unwrap().preamp + 9.0).abs() < 0.02);
    advance_to(&mut native, 0.99, &mut |_| Ok(()));
    assert!((native.current.as_ref().unwrap().preamp + 6.06).abs() < 0.02);
    advance_to(&mut native, 1.0, &mut |_| Ok(()));
    assert_eq!(native.current.as_ref().unwrap().preamp, -6.0);
}

#[test]
fn retained_inactive_high_band_does_not_attenuate_low_rate_edit() {
    let mut curve = EqCurve::default();
    curve.gains_db[9] = 12.0;
    let mut native = settled(&Payload::Eq(curve.clone()), 22_050);
    curve.gains_db[5] = 2.0;
    let payload = Payload::Eq(curve);
    let analysis = analyze(&payload, environment(22_050)).unwrap();
    native
        .apply(&payload, environment(22_050), &analysis, |_| Ok(()))
        .unwrap();
    advance_to(&mut native, 0.5, &mut |_| Ok(()));
    let current = native.current.as_ref().unwrap();
    assert!(
        current.preamp > -1.0,
        "inactive +12 dB must have no protection cost"
    );
    assert!((current.peak_gain - 1.0).abs() < 0.02);
}

#[test]
fn mixed_sign_edit_protects_each_command_state_with_manual_floor() {
    let mut curve = EqCurve {
        preamp_mode: PreampMode::Manual,
        gains_db: [12., -12., 12., -12., 12., -12., 12., -12., 12., -12.],
        ..EqCurve::default()
    };
    let mut native = settled(&Payload::Eq(curve.clone()), 48_000);
    let mut observed_gains = native.current.as_ref().unwrap().gains;
    let mut observed_preamp = native.current.as_ref().unwrap().preamp;
    curve.gains_db = curve.gains_db.map(|gain| -gain);
    let payload = Payload::Eq(curve);
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    native
        .apply(&payload, environment(48_000), &analysis, |_| Ok(()))
        .unwrap();
    for fraction in [0.25, 0.5, 0.75, 1.0] {
        advance_to(&mut native, fraction, &mut |args| {
            if args[1] == "echo_preamp" {
                observed_preamp = 20.0 * args[3].parse::<f64>().unwrap().log10();
            } else if let Some(index) = args[1].strip_prefix("echo_eq") {
                observed_gains[index.parse::<usize>().unwrap()] = args[3].parse().unwrap();
            }
            // Check all band centers and a logarithmic grid after every command.
            let coefficients = std::array::from_fn(|index| {
                crate::effects::math::Biquad::peaking(
                    BAND_FREQUENCIES[index],
                    48_000,
                    observed_gains[index],
                )
            });
            for frequency in BAND_FREQUENCIES
                .into_iter()
                .chain((0..100).map(|index| 20.0 * 1000.0_f64.powf(f64::from(index) / 99.0)))
            {
                assert!(
                    crate::effects::math::response_db(
                        &coefficients,
                        48_000,
                        frequency,
                        observed_preamp
                    ) < 0.000_01,
                    "manual transition budget exceeded"
                );
            }
            Ok(())
        });
    }
}

#[test]
fn rebuilt_mixed_curve_restores_cuts_before_boosts_even_when_disabling() {
    let payload = Payload::Eq(EqCurve {
        gains_db: [12., -12., 12., -12., 12., -12., 12., -12., 12., -12.],
        preamp_mode: PreampMode::Manual,
        ..EqCurve::default()
    });
    for enabled in [true, false] {
        let mut native = settled(&payload, 48_000);
        let analysis = analyze(&payload, environment(48_000)).unwrap();
        let mut gains = [0.0; 10];
        let mut preamp_db = 0.0;
        native.reconfirm();
        native.configured();
        let mut observe = |args: &[String]| {
            if args[1] == "echo_preamp" {
                preamp_db = 20.0 * args[3].parse::<f64>().unwrap().log10();
            } else if let Some(index) = args[1].strip_prefix("echo_eq") {
                gains[index.parse::<usize>().unwrap()] = args[3].parse().unwrap();
            }
            assert!(
                intermediate_eq_peak_db(gains, 48_000) + preamp_db <= 0.000_01,
                "reconfiguration must protect every accepted command state"
            );
            Ok(())
        };
        if enabled {
            native
                .apply(&payload, environment(48_000), &analysis, &mut observe)
                .unwrap();
        } else {
            native.bypass(&mut observe).unwrap();
        }
    }
}

#[test]
fn spatial_bypass_sends_intermediate_width_and_reaches_neutral_before_removal() {
    let payload = Payload::Spatial(Spatial::default());
    let mut native = settled(&payload, 48_000);
    native.bypass(|_| Ok(())).unwrap();
    let mut commands = Vec::new();
    advance_to(&mut native, 0.5, &mut |args| {
        commands.push(args.to_vec());
        Ok(())
    });
    let current = native.current.as_ref().unwrap();
    assert!((current.width - 1.125).abs() < 0.001);
    assert!(current.preamp > -1.0 && current.preamp < 0.0);
    assert!(commands.iter().any(|args| args[2] == "spatial:m"));
    commands.clear();
    advance_to(&mut native, 1.0, &mut |args| {
        commands.push(args.to_vec());
        Ok(())
    });
    let width = commands
        .iter()
        .position(|args| {
            args[0] == "af-command" && args[2] == "spatial:m" && args[3] == "1.000000000000000"
        })
        .unwrap();
    let preamp = commands
        .iter()
        .position(|args| {
            args[0] == "af-command" && args[2] == "preamp:volume" && args[3] == "1.000000000000000"
        })
        .unwrap();
    assert!(width < preamp && preamp < commands.len() - 1);
    assert_eq!(commands.last().unwrap()[..2], ["af", "remove"]);
}

#[test]
fn reenabling_partial_spatial_bypass_uses_current_width_without_rebuilding() {
    let payload = Payload::Spatial(Spatial::default());
    let mut native = settled(&payload, 48_000);
    native.bypass(|_| Ok(())).unwrap();
    advance_to(&mut native, 0.5, &mut |_| Ok(()));
    let width = native.current.as_ref().unwrap().width;
    let analysis = analyze(&payload, environment(48_000)).unwrap();
    native
        .apply(&payload, environment(48_000), &analysis, |args| {
            assert_eq!(
                args[0], "af-command",
                "partial bypass must not rebuild the chain"
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(native.ramp.as_ref().unwrap().from.width, width);
}

#[test]
fn bypass_command_failure_removes_owned_filters_and_never_confirms_success() {
    let payload = Payload::Spatial(Spatial::default());
    let mut native = settled(&payload, 48_000);
    let mut commands = Vec::new();
    let result = native.bypass(|args| {
        commands.push(args.to_vec());
        if args[0] == "af-command" {
            Err("injected bypass failure".to_owned())
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert!(native.current.is_none() && native.ramp.is_none() && native.labels.is_empty());
    assert_eq!(commands.last().unwrap()[..2], ["af", "remove"]);
}
