//! Formula and protection tests are independent of native audio measurements.
#![allow(clippy::float_cmp)] // Exact zeros and assigned half-step gains are the contract under test.
use super::*;
use crate::effects::{presets, EqCurve, Spatial};

fn environment(rate: u32) -> ProcessingEnvironment {
    ProcessingEnvironment {
        sample_rate: rate,
        channel_layout: ChannelLayout::Stereo,
    }
}

#[test]
fn center_gain_and_neutral_response_are_exact() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for (index, frequency) in BAND_FREQUENCIES.into_iter().enumerate() {
            if frequency > 0.45 * f64::from(rate) {
                continue;
            }
            for gain in [-12.0, 12.0] {
                let mut filters = [Biquad::IDENTITY; 10];
                filters[index] = Biquad::peaking(frequency, rate, gain);
                assert!((response_db(&filters, rate, frequency, 0.0) - gain).abs() < 0.00001);
            }
        }
        let result = analyze(&Payload::default(), environment(rate)).unwrap();
        assert_eq!(result.effective_preamp_db, 0.0);
    }
}

#[test]
fn pressure_and_low_rate_preserve_parameters() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for gains_db in [
            [12.0; 10],
            [-12.0; 10],
            [0., 0., 12., 12., 0., 0., 0., 0., 0., 0.],
        ] {
            let curve = EqCurve {
                gains_db,
                preamp_mode: PreampMode::Manual,
                requested_preamp_db: 0.0,
            };
            let result = analyze(&Payload::Eq(curve), environment(rate)).unwrap();
            assert!(result.effective_preamp_db.is_finite());
            assert!(result.peak_gain_db + result.effective_preamp_db <= 0.50001);
            assert_eq!(result.active_bands[9], rate != 22_050);
            if rate == 22_050 {
                assert_eq!(result.coefficients[9], Biquad::IDENTITY);
            }
        }
    }
}

#[test]
fn conservative_auto_preamp_keeps_modeled_steady_peak_within_limiter_budget() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for gains_db in [
            [12.0; 10],
            [-12.0; 10],
            [0., 12., 12., 0., -12., 0., 6., -6., 0., 12.],
        ] {
            let curve = EqCurve {
                gains_db,
                preamp_mode: PreampMode::Auto,
                requested_preamp_db: 0.0,
            };
            let analysis = analyze(&Payload::Eq(curve), environment(rate)).unwrap();
            assert!(analysis.effective_preamp_db.is_finite());
            assert!(analysis.peak_gain_db + analysis.effective_preamp_db <= 0.50001);
        }
    }
}

#[test]
fn published_design_comparison_is_reproducible() {
    let cases = [
        (
            "classical",
            [0., 0., 0., 0., 0., 0., 0.5, 0.5, 1., 1.5],
            1.57,
            0.61,
        ),
        (
            "electronic",
            [4., 5., 3., -1., -2., -1.5, 0., 2., 3., 2.],
            6.24,
            3.68,
        ),
        (
            "vocal",
            [-2., -2., -1.5, -0.5, 0., 1.5, 3., 1.5, -1.5, -1.],
            3.48,
            2.32,
        ),
        (
            "bass",
            [3., 6., 5., 1., 0., -0.5, -1., -1., -1.5, -2.],
            7.52,
            4.84,
        ),
    ];
    for (id, old_gains, old_peak, new_peak) in cases {
        let old = analyze(
            &Payload::Eq(EqCurve {
                gains_db: old_gains,
                ..EqCurve::default()
            }),
            environment(48_000),
        )
        .unwrap();
        let new = analyze(
            &presets::builtin(&crate::effects::PresetId::builtin(id))
                .unwrap()
                .payload,
            environment(48_000),
        )
        .unwrap();
        assert!((old.peak_gain_db - old_peak).abs() < 0.02);
        assert!((new.peak_gain_db - new_peak).abs() < 0.02);
        assert!((new.safe_preamp_db + new_peak).abs() < 0.02);
        assert!((new.effective_preamp_db - new.safe_preamp_db - 0.5).abs() < 0.001);
    }
}

#[test]
fn environment_spatial_and_validation_boundaries() {
    assert_eq!(
        analyze(&Payload::default(), environment(0)),
        Err(EffectsError::Unavailable)
    );
    let mut mono = environment(48_000);
    mono.channel_layout = ChannelLayout::Mono;
    assert!(analyze(&Payload::default(), mono).is_ok());
    assert_eq!(
        analyze(&Payload::Spatial(Spatial::default()), mono),
        Err(EffectsError::Unavailable)
    );
    let spatial = analyze(&Payload::Spatial(Spatial::default()), environment(48_000)).unwrap();
    assert!((spatial.peak_gain_db - 1.938_200_26).abs() < 0.000_001);
    assert!((spatial.effective_preamp_db + spatial.peak_gain_db - 0.5).abs() < 0.000_001);
    for invalid in [f64::NAN, f64::INFINITY, 12.5, 0.1] {
        let mut curve = EqCurve::default();
        curve.gains_db[0] = invalid;
        assert_eq!(curve.validate(), Err(EffectsError::InvalidParameter));
    }
}

#[test]
fn transition_bound_protects_mixed_sign_interpolation() {
    let left = Payload::Eq(EqCurve {
        gains_db: [12., -12., 12., -12., 12., -12., 12., -12., 12., -12.],
        ..EqCurve::default()
    });
    let right = Payload::Eq(EqCurve {
        gains_db: [-12., 12., -12., 12., -12., 12., -12., 12., -12., 12.],
        ..EqCurve::default()
    });
    let bound = transition_preamp_db(&left, &right, environment(48_000)).unwrap();
    // All ten bands alternate sign, so every band's positive endpoint max is
    // 12 dB and the intermediate bound is the full 120 dB. The transition shares
    // AUTO_HEADROOM_DB with the steady-state preamp; it used to add a separate
    // 1.0 dB, which made the transition and steady state disagree.
    assert_eq!(bound, -(120.0 + AUTO_HEADROOM_DB));
    assert_eq!(
        transition_preamp_db(
            &Payload::default(),
            &Payload::default(),
            environment(48_000)
        )
        .unwrap(),
        0.0
    );
    for step in 0..=10 {
        let proportion = f64::from(step) / 10.0;
        // Sample valid 0.5 dB intermediate curves at quarter intervals.
        let gain = if step % 5 == 0 {
            12.0 * 2.0f64.mul_add(-proportion, 1.0)
        } else {
            0.0
        };
        let intermediate = Payload::Eq(EqCurve {
            gains_db: std::array::from_fn(|index| if index % 2 == 0 { gain } else { -gain }),
            ..EqCurve::default()
        });
        let analysis = analyze(&intermediate, environment(48_000)).unwrap();
        assert!(analysis.peak_gain_db + bound <= -1.0);
    }
}
